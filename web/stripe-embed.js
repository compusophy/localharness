// Stripe Elements on-ramp shim. The wasm app fetches a PaymentIntent
// `client_secret` from the proxy, swaps in our branded modal (maud), then calls
// `window.lhBuyLh(optsJson)` to mount Stripe's native Elements inside it:
//   #lh-express  — Express Checkout (Link / Apple Pay / Google Pay), one-click,
//                  SELF-CONFIRMING via its own `confirm` event.
//   #lh-payment  — Payment Element (Link "use this card" inline + card).
//
// There is NO custom pay button: the user pays by clicking Stripe's OWN buttons
// (the green Link button or the inline "use this card"). Because those confirm
// the PaymentIntent directly — sometimes without our confirmPayment call ever
// resolving in our code — the SUCCESS + on-chain mint are driven by a poll on
// the Rust side: it watches `window.lhPaymentStatus()` and, when the
// PaymentIntent is `succeeded`, mints via /stripe/finalize with a FRESHLY signed
// token (so a slow payer's modal-open token can't go stale and silently 401 the
// mint — the bug that charged a card but credited no $LH). `window.lhBuySuccess`
// flips the modal to the done state once the mint lands.
//
// The publishable key is PUBLIC by design (Stripe pk_live_). All imperative
// Stripe.js wiring lives here in the JS glue layer (like boot.js).
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

  // Confirm the PaymentIntent. Required inside the Express Checkout `confirm`
  // event (the green Link/wallet button) so the charge actually goes through.
  // The Payment Element's inline Link "use this card" confirms on its own and
  // never calls this. Either way the Rust poll catches `succeeded` and mints.
  function confirmPay() {
    if (!state) return Promise.resolve();
    var o = state.opts;
    var returnUrl = o.returnUrl || (window.location.origin + window.location.pathname + '?bought=1');
    showError('');
    return state.stripe
      .confirmPayment({
        elements: state.elements,
        clientSecret: o.clientSecret,
        confirmParams: { return_url: returnUrl },
        redirect: 'if_required',
      })
      .then(function (res) {
        if (res && res.error) showError(res.error.message || 'payment failed');
        // Success is handled by the Rust status poll, not here.
      })
      .catch(function (e) { showError((e && e.message) || 'payment error'); });
  }

  // Polled by the Rust reactivity loop. Resolves the PaymentIntent status
  // ('requires_payment_method' | 'processing' | 'succeeded' | ...). Client-side
  // (publishable-key) call, so it's cheap to poll and works for EVERY confirm
  // path — popup Link, inline "use this card", or the express button.
  window.lhPaymentStatus = function () {
    if (!state) return Promise.resolve('none');
    return state.stripe
      .retrievePaymentIntent(state.opts.clientSecret)
      .then(function (r) { return (r && r.paymentIntent && r.paymentIntent.status) || 'unknown'; })
      .catch(function () { return 'error'; });
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
      var stripe = Stripe(PK);
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
      try {
        var express = elements.create('expressCheckout', {
          paymentMethods: { applePay: 'auto', googlePay: 'auto' },
        });
        express.on('confirm', function () { confirmPay(); });
        express.mount('#lh-express');
      } catch (e) {
        var slot = byId('lh-express');
        if (slot) slot.style.display = 'none';
      }

      // Payment Element — Link inline "use this card" (self-confirming) + card.
      var payment = elements.create('payment', {
        layout: { type: 'accordion', defaultCollapsed: true, radios: true, spacedAccordionItems: false },
        paymentMethodOrder: ['card', 'link'],
        wallets: { applePay: 'never', googlePay: 'never' },
      });
      payment.mount('#lh-payment');
    });
  };

  window.lhUnmountCheckout = function () { state = null; };

  // Warm Stripe.js on page load so mounting the Elements is INSTANT when the
  // user taps "create agent" (instead of loading the ~heavy Stripe.js on the
  // critical path mid-checkout). preconnect to js.stripe.com is in index.html.
  loadStripe().catch(function () {});
})();
