// Static server + report sink for the iOS-simulator WebKit E2E
// (.github/workflows/ios-webkit-e2e.yml). The simulator shares the Mac host's
// network stack, so MobileSafari's http://localhost:<port> reaches this
// server. Serves the mirrored deployed bundle + e2e-ios-probe.html, and the
// probe page POSTs its verdict to /lh-e2e-report?phase=N — persisted as
// e2e-report-phaseN.json in the CWD, which the workflow polls for and asserts
// on (no DOM automation, no Appium: the page self-reports).
//
//   node scripts/tab-e2e/ios-sim-server.mjs <dir> <port>
import { createServer } from 'node:http';
import { readFile, writeFile } from 'node:fs/promises';
import { join, extname, normalize } from 'node:path';

const [dir = '_site', port = '8080'] = process.argv.slice(2);
const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'application/javascript',
  '.mjs': 'application/javascript',
  '.css': 'text/css',
  '.wasm': 'application/wasm',
  '.json': 'application/json',
  '.webmanifest': 'application/manifest+json',
  '.txt': 'text/plain; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.md': 'text/plain; charset=utf-8',
};

createServer(async (req, res) => {
  const url = new URL(req.url, 'http://localhost');
  if (req.method === 'POST' && url.pathname === '/lh-e2e-report') {
    let body = '';
    for await (const c of req) body += c;
    const phase = (url.searchParams.get('phase') || '0').replace(/[^0-9]/g, '');
    await writeFile(`e2e-report-phase${phase}.json`, body || '{}');
    console.log(`[report phase ${phase}]`, body);
    res.writeHead(204).end();
    return;
  }
  let p = url.pathname === '/' ? '/index.html' : url.pathname;
  p = normalize(p).replace(/^([.][.][/\\])+/, ''); // no traversal
  try {
    const data = await readFile(join(dir, p));
    res.writeHead(200, {
      'content-type': MIME[extname(p).toLowerCase()] || 'application/octet-stream',
      'cache-control': 'no-store',
    });
    res.end(data);
  } catch {
    res.writeHead(404).end('not found');
  }
}).listen(Number(port), () => console.log(`ios-sim e2e server: serving ${dir} on :${port}`));
