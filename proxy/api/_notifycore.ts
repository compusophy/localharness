// _notifycore.ts — the ONE simple owner-push delivery core: VAPID config →
// enrolled subscriptions → best-effort send. Extracted from scheduler.ts's
// private copy (telemetry #78's named first step) so every server-side
// "buzz this owner" caller shares one implementation — the scheduler today,
// the inbound-email wake when it earns its way in (it still needs a read
// path + a non-spoofable mailbox before that wire is pulled).
//
// DELIBERATELY SIMPLER than api/notify.ts's endpoint flow: notify.ts meters
// the sender, prunes dead endpoints via the DETAILED send results, and
// surfaces a 502 with the pre-prune dead count — semantics an internal
// best-effort push must not drag in. Both share the primitives underneath
// (_pushstore + _webpush); this module owns only the simple composition.
import { sendWebPushAll } from './_webpush';
import { storePushSubs } from './_pushstore';

/**
 * Web-Push a `{title, body}` JSON (rendered by web/sw.js) to every device the
 * OWNER address has enrolled. Returns true iff at least one push service
 * accepted. NEVER throws: missing VAPID env, no subscriptions, and send
 * failures all resolve to false — an internal notification must never fail
 * (or re-fire) the operation that triggered it. Bounded: one store read +
 * 5s-capped POSTs.
 */
export async function deliverOwnerPush(
  owner: string,
  title: string,
  body: string,
): Promise<boolean> {
  const publicKey = process.env.VAPID_PUBLIC_KEY;
  const privateKey = process.env.VAPID_PRIVATE_KEY;
  const subject = process.env.VAPID_SUBJECT;
  if (!publicKey || !privateKey || !subject) return false; // push not configured
  const subs = await storePushSubs(owner); // never throws ([] on failure)
  if (subs.length === 0) return false; // owner never enabled notifications
  const sent = await sendWebPushAll(subs, JSON.stringify({ title, body }), {
    publicKey,
    privateKey,
    subject,
  });
  return sent > 0;
}
