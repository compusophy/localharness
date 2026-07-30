## Goal

Compile a rustlite cartridge and publish it as a subdomain's public face.

## Steps

1. Write `app.rl` with a `frame(t)` that clears, draws, and presents.
2. Verify it compiles: `localharness compile app.rl --host-calls`.
3. Publish: `localharness publish <name> app.rl` (sponsored setMetadata; sets `public_face=app`).
