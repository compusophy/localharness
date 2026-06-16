// Stripe Embedded Checkout shim. The wasm app fetches a Checkout `client_secret`
// from the proxy, swaps in our branded modal (maud), then calls
// `window.lhBuyLh(clientSecret)` here to mount Stripe's embedded checkout INSIDE
// the modal (#stripe-checkout-mount) — no redirect. On payment completion the
// modal flips to a success state; the proxy webhook does the on-chain mint.
//
// The publishable key is PUBLIC by design (Stripe pk_live_). All the imperative
// Stripe.js wiring lives here in the JS glue layer (like boot.js), keeping it
// out of the Rust app code.
(function () {
  var PK =
    'pk_live_51Tiu4kLz8dIS1FUar4pfDglshUY9Fw9xSPEq4aSc2dmx14X1gk4evtWtEVP2kAXB87f5HVEKIRLKnuFluRI3IGpw004331RqyZ';
  var stripeLoad = null;
  var current = null;

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

  // Mount embedded checkout into #stripe-checkout-mount. Returns a rejected
  // promise the caller can surface if Stripe.js / init fails.
  window.lhBuyLh = function (clientSecret) {
    return loadStripe().then(function (Stripe) {
      window.lhUnmountCheckout();
      var stripe = Stripe(PK);
      return stripe
        .initEmbeddedCheckout({
          clientSecret: clientSecret,
          onComplete: function () {
            var done = document.getElementById('buy-modal-done');
            var mount = document.getElementById('stripe-checkout-mount');
            if (mount) mount.style.display = 'none';
            if (done) done.style.display = 'block';
          },
        })
        .then(function (checkout) {
          current = checkout;
          checkout.mount('#stripe-checkout-mount');
        });
    });
  };

  window.lhUnmountCheckout = function () {
    if (current) {
      try { current.destroy(); } catch (e) {}
      current = null;
    }
  };
})();
