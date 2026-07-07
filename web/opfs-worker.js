// opfs-worker.js — the WebKit-safe OPFS WRITE BROKER (a dedicated worker).
//
// WHY THIS EXISTS (the iOS fix):
//   Safari/WebKit had NO `FileSystemFileHandle.createWritable()` until
//   Safari 26 (Sept 2025) — on iOS ≤ 18 (iPhone XR/XS forever) the method is
//   `undefined`, so the wasm app's main-thread OPFS write DIED mid-onboarding
//   ("not available on iOS" gate, 2026-06-18). The API that HAS worked on
//   WebKit since iOS 15.2 is `createSyncAccessHandle` — but it is WORKER-ONLY.
//   So the Rust side (src/filesystem/opfs.rs) routes every write on WebKit
//   engines through this broker instead. One codepath covers iOS 15.2→26.x,
//   Chrome-on-iOS (CriOS = WebKit), and embedded WKWebViews. This is the same
//   route SQLite-wasm / RxDB ship on WebKit.
//
// PROTOCOL: { id, path, bytes:ArrayBuffer } → { id, ok, err?, unsupported? }.
//   `unsupported:true` = this engine lacks createSyncAccessHandle too; the
//   Rust caller falls back to its (timeout-bounded) main-thread path.
//
// PARITY: path semantics MIRROR src/filesystem/opfs.rs::split_path — split on
//   '/', drop empty segments and "." — and parent dirs are created on demand
//   (resolve_parent create=true). Guarded by tests/opfs_worker_parity.rs.
//
// Ops run STRICTLY SEQUENTIALLY (promise-chain queue): sync access handles
// take an EXCLUSIVE per-file lock, and interleaved handlers at await points
// would contend. Handles are opened per-op and closed in `finally` — nothing
// stays locked across ops (iOS kills background workers; a parked handle
// would strand the lock).

let tail = Promise.resolve();

self.onmessage = (e) => {
  const { id, path, bytes } = e.data;
  tail = tail.then(() => writeOnce(path, bytes)).then(
    () => self.postMessage({ id, ok: true }),
    (err) => self.postMessage({
      id,
      ok: false,
      err: String(err),
      unsupported: /createSyncAccessHandle|not a function|undefined is not/i.test(String(err)),
    }),
  );
};

async function writeOnce(path, bytes) {
  const parts = path.split('/').filter((s) => s !== '' && s !== '.');
  if (parts.length === 0) throw new Error('write: empty path');
  let dir = await navigator.storage.getDirectory();
  for (const p of parts.slice(0, -1)) {
    dir = await dir.getDirectoryHandle(p, { create: true });
  }
  const fh = await dir.getFileHandle(parts[parts.length - 1], { create: true });
  if (typeof fh.createSyncAccessHandle !== 'function') {
    throw new Error('createSyncAccessHandle unavailable');
  }
  // A stale lock (previous tab/worker killed before close — WebKit bug
  // 239614) throws here; retry ONCE after a beat before giving up.
  let handle;
  try {
    handle = await fh.createSyncAccessHandle();
  } catch (first) {
    await new Promise((r) => setTimeout(r, 150));
    handle = await fh.createSyncAccessHandle();
  }
  try {
    handle.truncate(0);
    handle.write(new Uint8Array(bytes), { at: 0 });
    handle.flush();
  } finally {
    handle.close();
  }
}
