// Wasm-FREE Stripe checkout page logic (see web/pay.html header). The heavy
// localharness wasm app (~4.6 MB) and Stripe's Payment Element iframes can't
// coexist in one iOS Safari WebContent process without an out-of-memory kill
// (gray screen + reload ~10s after the card mounts). So the card form lives on
// THIS page, which loads NO wasm: the app fetches the PaymentIntent
// client_secret (it holds the identity key — this page never sees it), navigates
// here with it in the URL FRAGMENT, we mount Stripe + confirm the card, then
// navigate back to the app with ?bought=1&pi=… for the on-chain mint.
//
// The publishable key is PUBLIC by design (pk_live_). External file (not inline)
// so the page stays compatible with `script-src 'self'`.
(function () {
  'use strict';
  var PK =
    'pk_live_51Tiu4kLz8dIS1FUar4pfDglshUY9Fw9xSPEq4aSc2dmx14X1gk4evtWtEVP2kAXB87f5HVEKIRLKnuFluRI3IGpw004331RqyZ';

  function byId(id) { return document.getElementById(id); }
  function showError(m) { var e = byId('lh-pay-error'); if (e) e.textContent = m || ''; }

  // Query carries the display amount + payment_intent + onboarding flag; the
  // FRAGMENT carries the Stripe client_secret (fragments aren't sent to servers
  // or put in Referer/logs).
  var query = new URLSearchParams(window.location.search || '');
  var frag = new URLSearchParams((window.location.hash || '').replace(/^#/, ''));
  var clientSecret = frag.get('cs') || '';
  var pi = query.get('pi') || '';
  var onboarding = query.get('ob') === '1';
  var cents = parseInt(query.get('usd') || '0', 10) || 0;
  var dollars = Math.round(cents / 100);
  var payLabel = 'pay $' + dollars;

  // pay.html ships on every origin, so this is same-origin — return to THIS
  // origin's app (apex for onboarding, the subdomain for an admin top-up).
  var returnUrl =
    window.location.origin + '/?bought=1&pi=' + encodeURIComponent(pi) + '&ob=' + (onboarding ? '1' : '0');

  var amtEl = byId('pay-amount');
  if (amtEl) amtEl.textContent = '$' + dollars + ' → ' + cents + ' $LH';

  if (!clientSecret) {
    showError('this checkout link is incomplete — go back and tap buy again');
    return;
  }

  function loadStripe() {
    if (window.Stripe) return Promise.resolve(window.Stripe);
    return new Promise(function (resolve, reject) {
      var s = document.createElement('script');
      s.src = 'https://js.stripe.com/v3/';
      s.onload = function () { resolve(window.Stripe); };
      s.onerror = function () { reject(new Error('failed to load Stripe.js')); };
      document.head.appendChild(s);
    });
  }

  loadStripe()
    .then(function (Stripe) {
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
      var elements = stripe.elements({ clientSecret: clientSecret, appearance: appearance });
      // Payment Element, card-only (the PaymentIntent is card+link-only at the proxy).
      var payment = elements.create('payment', {
        layout: { type: 'accordion', defaultCollapsed: false, radios: true, spacedAccordionItems: false },
        paymentMethodOrder: ['card'],
      });
      payment.mount('#lh-payment');

      var btn = byId('lh-pay-button');
      if (!btn) return;
      btn.textContent = payLabel;
      btn.style.display = '';
      btn.disabled = false;
      btn.onclick = function () {
        btn.disabled = true;
        btn.textContent = 'processing…';
        showError('');
        stripe
          .confirmPayment({
            elements: elements,
            confirmParams: { return_url: returnUrl },
            redirect: 'if_required',
          })
          .then(function (result) {
            if (result && result.error) {
              showError(result.error.message || 'payment failed');
              btn.disabled = false;
              btn.textContent = payLabel;
              return;
            }
            // Card resolves inline (no redirect) → navigate back to the app, which
            // mints the now-paid PaymentIntent. Redirect-based methods are sent to
            // return_url by Stripe itself. The webhook is the durable backstop.
            var done = byId('pay-done');
            if (done) done.style.display = 'block';
            window.location.assign(returnUrl);
          })
          .catch(function (e) {
            showError((e && e.message) || 'payment error');
            btn.disabled = false;
            btn.textContent = payLabel;
          });
      };
    })
    .catch(function () {
      showError('could not load secure checkout — check your connection and try again');
    });
})();
