// Stripe Elements on-ramp shim. The wasm app fetches a PaymentIntent
// `client_secret` from the proxy, swaps in our branded card form (maud), then
// calls `window.lhBuyLh(optsJson)` to mount Stripe's Payment Element into
// `#lh-payment` (card + inline Link). The PaymentIntent is card+link-only, and we
// mount ONLY the Payment Element (no Express Checkout Element — its express
// buttons surfaced useless bank/Klarna options and tangled the confirm).
//
// The Payment Element renders NO button of its own, so #lh-pay-button (revealed +
// wired in lhBuyLh) is OUR submit control — it calls stripe.confirmPayment via the
// CANONICAL clientSecret flow ({elements, confirmParams, redirect:'if_required'},
// no elements.submit(), no clientSecret arg). On success confirmPayment resolves
// with {paymentIntent:'succeeded'} → handleSucceeded flips the modal to done
// immediately. A JS status poll (`window.lhWatchPayment`, setInterval over
// `retrievePaymentIntent`) is the BACKSTOP for redirect/late-settle returns and
// when the PaymentIntent is `succeeded`, calls `window.lh_payment_succeeded`
// (the wasm export, wired in boot.js) which mints via /stripe/finalize with a
// FRESHLY signed token (so a slow payer's modal-open token can't go stale and
// silently 401 the mint — the bug that charged a card but credited no $LH). The
// poll lives in JS, NOT wasm: the old wasm JsFuture + timer loop re-entered the
// wasm-bindgen single-thread executor on iOS WebKit ("already mutably borrowed:
// BorrowError") and killed the app mid-checkout. `window.lhBuySuccess` flips the
// modal to the done state once the mint lands.
//
// The publishable key is PUBLIC by design (Stripe pk_live_). All imperative
// Stripe.js wiring lives here in the JS glue layer (like boot.js).
(function () {
  var PK =
    'pk_live_51Tiu4kLz8dIS1FUar4pfDglshUY9Fw9xSPEq4aSc2dmx14X1gk4evtWtEVP2kAXB87f5HVEKIRLKnuFluRI3IGpw004331RqyZ';
  var stripeLoad = null;
  var stripeInstance = null; // ONE Stripe(PK) instance, reused across buys
  var state = null; // { stripe, elements, opts }
  var watchTimer = null; // setInterval id for the JS payment-status poll
  var watchOpts = null; // {payment_intent, onboarding, lh_label} shared with confirmPay
  var handledPI = null; // payment_intent already resolved — handleSucceeded is once-per-PI

  // Stop the JS payment-status poll (idempotent). Called on success, on
  // teardown (`lhUnmountCheckout`), and after the time cap — the interval must
  // never leak past a closed/torn-down checkout.
  function stopWatch() {
    if (watchTimer !== null) {
      clearInterval(watchTimer);
      watchTimer = null;
    }
  }

  // Drive the post-payment UI + wasm mint EXACTLY ONCE per PaymentIntent. Called
  // from BOTH confirmPay's success (immediate — no redirect) and the status poll
  // (a backstop for redirect / late-settle returns). A plain buy flips the modal
  // to "received" right here in JS, so the pay button never hangs on "processing…"
  // waiting on the slow wasm finalize loop; onboarding defers to the wasm side (it
  // draws the seed-backup screen). The wasm callback still runs for the balance
  // refresh + (onboarding) seed persist.
  function handleSucceeded() {
    var o = watchOpts;
    if (!o || handledPI === o.payment_intent) return;
    handledPI = o.payment_intent;
    stopWatch();
    if (!o.onboarding && typeof window.lhBuySuccess === 'function') {
      window.lhBuySuccess('✓ payment received — your $LH is being credited');
    }
    if (typeof window.lh_payment_succeeded === 'function') {
      window.lh_payment_succeeded(o.payment_intent, !!o.onboarding, o.lh_label || '');
    }
  }

  function loadStripe() {
    if (window.Stripe) return Promise.resolve(window.Stripe);
    if (stripeLoad) return stripeLoad;
    stripeLoad = new Promise(function (resolve, reject) {
      var s = document.createElement('script');
      s.src = 'https://js.stripe.com/v3/';
      s.onload = function () { resolve(window.Stripe); };
      s.onerror = function () { reject(new Error('failed to load Stripe.js')); };
      document.head.appendChild(s);
    });
    return stripeLoad;
  }

  function byId(id) { return document.getElementById(id); }
  function showError(msg) { var el = byId('lh-pay-error'); if (el) el.textContent = msg || ''; }

  // Confirm the PaymentIntent for the card pay button. The CANONICAL Payment
  // Element clientSecret flow (Stripe docs, verbatim): pass `elements` (the
  // clientSecret is already on it) + `confirmParams`, with redirect:'if_required'.
  // Do NOT call elements.submit() and do NOT pass clientSecret here — that
  // combination (a relic of the now-removed Express Checkout Element) is what
  // wedged the confirm. A card resolves inline with `{ paymentIntent }`; status
  // 'succeeded' drives the done state immediately via handleSucceeded. Resolves
  // `{ ok }` so the pay button re-enables itself on failure.
  function confirmPay() {
    if (!state) return Promise.resolve({ ok: false });
    var o = state.opts;
    var returnUrl = o.returnUrl || (window.location.origin + window.location.pathname + '?bought=1');
    showError('');
    return state.stripe
      .confirmPayment({
        elements: state.elements,
        confirmParams: { return_url: returnUrl },
        redirect: 'if_required',
      })
      .then(function (result) {
        if (result && result.error) { showError(result.error.message || 'payment failed'); return { ok: false }; }
        // No error → the payment was accepted. Unstick the UI NOW regardless of
        // status: a card resolves 'succeeded', but a bank-backed method (Link →
        // bank account) resolves 'processing' — gating on 'succeeded' left those
        // hung on "processing…". The webhook mints on the real
        // payment_intent.succeeded; handleSucceeded is idempotent per PI.
        handleSucceeded();
        return { ok: true };
      })
      .catch(function (e) { showError((e && e.message) || 'payment error'); return { ok: false }; });
  }

  // Watch the PaymentIntent until it `succeeded`, then mint via wasm. Runs the
  // status poll IN JS (the iOS BorrowError fix — see the header) using the
  // publishable-key `retrievePaymentIntent`, cheap and covering EVERY confirm
  // path (popup Link, inline "use this card", express button). On success it
  // stops the interval and calls `window.lh_payment_succeeded` (the wasm export,
  // wired in boot.js); the wasm side mints with a freshly signed token. The
  // interval is cleared on success, when the checkout was torn down (`state`
  // null), and after a ~6-min cap. `lhUnmountCheckout` also stops it.
  window.lhWatchPayment = function (optsJson) {
    var o;
    try { o = typeof optsJson === 'string' ? JSON.parse(optsJson) : optsJson; }
    catch (e) { return; }
    if (!o || !o.payment_intent) return;
    watchOpts = o; // share onboarding/payment_intent/lh_label with confirmPay
    handledPI = null; // new payment session
    stopWatch(); // never run two watchers at once
    var ticks = 0;
    var maxTicks = 120; // 120 * 3s = 6 min cap
    watchTimer = setInterval(function () {
      // Checkout torn down or capped → stop; the proxy webhook is the backstop.
      if (!state || ++ticks > maxTicks) {
        stopWatch();
        return;
      }
      state.stripe
        .retrievePaymentIntent(state.opts.clientSecret)
        .then(function (r) {
          var st = r && r.paymentIntent && r.paymentIntent.status;
          if (st === 'succeeded') {
            handleSucceeded();
          }
        })
        .catch(function () { /* transient — keep polling until the cap */ });
    }, 3000);
  };

  // Flip the modal to the done state once the on-chain mint lands. `msg` is the
  // confirmation line (e.g. "✓ 0.67 $LH added").
  window.lhBuySuccess = function (msg) {
    var region = byId('lh-pay-region');
    var done = byId('buy-modal-done');
    if (region) region.style.display = 'none';
    if (done) {
      if (msg) done.textContent = msg;
      done.style.display = 'block';
    }
  };

  // Surface a LOUD post-payment error in the modal (e.g. the seed persist
  // failed after a confirmed mint). Keeps the modal open so the user can act
  // (reveal/back up their seed, reload) — never a silent swallow of a paid PI.
  window.lhBuyError = function (msg) {
    showError(msg || 'something went wrong — do not close this tab');
  };

  window.lhBuyLh = function (optsJson) {
    var o;
    try { o = typeof optsJson === 'string' ? JSON.parse(optsJson) : optsJson; }
    catch (e) { return Promise.reject(e); }
    return loadStripe().then(function (Stripe) {
      window.lhUnmountCheckout();
      // Reuse ONE Stripe instance across buys — constructing Stripe(PK) repeatedly
      // hinders performance (each instance re-inits fraud/telemetry signals).
      if (!stripeInstance) stripeInstance = Stripe(PK);
      var stripe = stripeInstance;
      var appearance = {
        theme: 'night',
        variables: {
          colorPrimary: '#ffffff',
          colorBackground: '#0a0a0a',
          colorText: '#e6e6e6',
          colorTextSecondary: '#9a9a9a',
          colorDanger: '#ff6b6b',
          fontFamily: 'IBM Plex Mono, ui-monospace, monospace',
          borderRadius: '2px',
          spacingUnit: '3px',
        },
      };
      var elements = stripe.elements({ clientSecret: o.clientSecret, appearance: appearance });
      state = { stripe: stripe, elements: elements, opts: o };

      // Payment Element ONLY (card + inline Link) — the canonical clientSecret
      // flow. NO Express Checkout Element: its auto-detected express buttons
      // surfaced the useless bank/Klarna options, and pairing it with
      // clientSecret + confirmPayment is what wedged the confirm (the submit()
      // tangle). The PaymentIntent is card+link-only (proxy), so nothing else shows.
      var payment = elements.create('payment', {
        layout: { type: 'accordion', defaultCollapsed: false, radios: true, spacedAccordionItems: false },
        paymentMethodOrder: ['card'],
      });
      payment.mount('#lh-payment');

      // OUR submit button for the Payment Element (card / inline Link): the
      // Payment Element renders no button of its own, so without this the user
      // fills the card and has nothing to click to pay. (The Express Checkout
      // button above self-confirms and needs none.) Reveal it now that the form
      // is mounted and wire it to confirmPayment; the status poll handles success.
      var payBtn = byId('lh-pay-button');
      if (payBtn) {
        if (o.payLabel) payBtn.textContent = o.payLabel;
        var payLabel = payBtn.textContent;
        payBtn.style.display = '';
        payBtn.disabled = false;
        payBtn.onclick = function () {
          payBtn.disabled = true;
          payBtn.textContent = 'processing…';
          confirmPay().then(function (r) {
            // Success → the status poll flips the modal to done; keep it disabled.
            // Failure (declined / incomplete) → re-enable so the user can retry.
            if (!r || !r.ok) { payBtn.disabled = false; payBtn.textContent = payLabel; }
          });
        };
      }
    });
  };

  window.lhUnmountCheckout = function () { stopWatch(); state = null; watchOpts = null; };

  // Warm Stripe.js on page load so mounting the Elements is INSTANT when the
  // user taps "create agent" (instead of loading the ~heavy Stripe.js on the
  // critical path mid-checkout). preconnect to js.stripe.com is in index.html.
  loadStripe().catch(function () {});
})();
