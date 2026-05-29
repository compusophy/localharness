// Bootstraps the wasm bundle. Kept as an external module (not inline) so
// the Content-Security-Policy can use `script-src 'self'` without
// `'unsafe-inline'`. All application code lives in the wasm bundle; this
// is the only JavaScript in the project.
import init from "./pkg/localharness.js";
await init();
