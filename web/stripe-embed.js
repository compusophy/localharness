// Stripe Elements on-ramp shim. The wasm app fetches a PaymentIntent
// `client_secret` from the proxy, swaps in our branded modal (maud), then calls
// `window.lhBuyLh(optsJson)` here to mount a COMPACT Stripe Elements form inside
// it (#lh-express + #lh-payment) — an Express Checkout button (Link / Apple Pay
// / Google Pay, one-click) on top, then a Payment Element (card, collapsed) below.
//
// This replaces Stripe Embedded Checkout (`initEmbeddedCheckout`), whose entire
// hosted checkout page rendered as a tall nested iframe inside our modal — the
// slow, unscrollable "modal-in-modal" with 20 fields on open. Elements is a short
// form that loads fast and scrolls inside a normal modal.
//
// On success we flip the modal to a done state and POST /stripe/finalize to mint
// immediately (the proxy webhook is the durable backstop). The publishable key is
// PUBLIC by design (Stripe pk_live_). All imperative Stripe.js wiring lives here
// in the JS glue layer (like boot.js), keeping it out of the Rust app code.
(function () {
  var PK =
    'pk_live_51Tiu4kLz8dIS1FUar4pfDglshUY9Fw9xSPEq4aSc2dmx14X1gk4evtWtEVP2kAXB87f5HVEKIRLKnuFluRI3IGpw004331RqyZ';
  var stripeLoad = null;
  var state = null; // { stripe, elements, opts }

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

  function showDone() {
    var region = byId('lh-pay-region');
    var done = byId('buy-modal-done');
    if (region) region.style.display = 'none';
    if (done) done.style.display = 'block';
  }

  function setBusy(b) {
    var btn = byId('lh-pay-btn');
    if (!btn) return;
    btn.disabled = b;
    btn.textContent = b ? 'processing…' : ((state && state.opts && state.opts.payLabel) || 'pay');
  }

  // Best-effort instant mint after a confirmed payment. Idempotent on-chain, so a
  // 401 (stale auth token on a slow payer) or any failure just falls back to the
  // webhook backstop — never double-mints.
  function finalize() {
    var o = state && state.opts;
    if (!o || !o.finalizeUrl || !o.paymentIntentId) return;
    try {
      fetch(o.finalizeUrl, {
        method: 'POST',
        headers: { 'content-type': 'application/json', 'x-goog-api-key': o.authToken || '' },
        body: JSON.stringify({ payment_intent: o.paymentIntentId }),
      }).catch(function () {});
    } catch (e) { /* ignore */ }
  }

  function confirmPay() {
    if (!state) return Promise.resolve();
    var stripe = state.stripe, elements = state.elements, o = state.opts;
    setBusy(true);
    showError('');
    var returnUrl = o.returnUrl || (window.location.origin + window.location.pathname + '?bought=1');
    return stripe
      .confirmPayment({
        elements: elements,
        clientSecret: o.clientSecret,
        confirmParams: { return_url: returnUrl },
        redirect: 'if_required',
      })
      .then(function (res) {
        setBusy(false);
        if (res.error) { showError(res.error.message || 'payment failed'); return; }
        if (res.paymentIntent && res.paymentIntent.status === 'succeeded') {
          showDone();
          finalize();
        } else {
          showError('payment not completed');
        }
      })
      .catch(function (e) {
        setBusy(false);
        showError((e && e.message) || 'payment error');
      });
  }

  // Mount the Elements form. `optsJson` carries clientSecret + paymentIntentId +
  // finalizeUrl + authToken + payLabel (built in Rust).
  window.lhBuyLh = function (optsJson) {
    var o;
    try { o = typeof optsJson === 'string' ? JSON.parse(optsJson) : optsJson; }
    catch (e) { return Promise.reject(e); }
    return loadStripe().then(function (Stripe) {
      window.lhUnmountCheckout();
      var stripe = Stripe(PK);
      // Brutalist monochrome to match the app chrome.
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

      // Express Checkout (Link / Apple Pay / Google Pay) — one-click, on top.
      // Auto-hides itself + the "or pay with card" divider when nothing is
      // eligible (a brand-new visitor with no wallet), leaving just the card form.
      try {
        var express = elements.create('expressCheckout', {
          paymentMethods: { applePay: 'auto', googlePay: 'auto' },
        });
        express.on('ready', function (ev) {
          var methods = ev && ev.availablePaymentMethods;
          var has = methods && Object.keys(methods).length > 0;
          if (!has) {
            var slot = byId('lh-express');
            var div = byId('lh-or-card');
            if (slot) slot.style.display = 'none';
            if (div) div.style.display = 'none';
          }
        });
        express.on('confirm', function () { confirmPay(); });
        express.mount('#lh-express');
      } catch (e) {
        var slot2 = byId('lh-express');
        var div2 = byId('lh-or-card');
        if (slot2) slot2.style.display = 'none';
        if (div2) div2.style.display = 'none';
      }

      // Payment Element — card first, collapsed accordion so NO fields show on
      // open (you pick "Card" to expand ~4 fields, not 20). Link is kept as a
      // fallback row here for visitors who don't get the express Link button.
      // Apple/Google Pay live in the express button, not duplicated here.
      var payment = elements.create('payment', {
        layout: { type: 'accordion', defaultCollapsed: true, radios: true, spacedAccordionItems: false },
        paymentMethodOrder: ['card', 'link'],
        wallets: { applePay: 'never', googlePay: 'never' },
      });
      payment.mount('#lh-payment');

      var btn = byId('lh-pay-btn');
      if (btn) {
        btn.disabled = false;
        btn.textContent = o.payLabel || 'pay';
        btn.onclick = function () { confirmPay(); };
      }
    });
  };

  window.lhUnmountCheckout = function () {
    // Elements are torn down with the modal subtree when Rust removes #buy-modal;
    // just drop our handle so a re-open starts clean.
    state = null;
  };
})();
