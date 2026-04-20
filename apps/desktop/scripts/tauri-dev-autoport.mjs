import fs from 'node:fs/promises';
import net from 'node:net';
import path from 'node:path';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const appDir = path.resolve(__dirname, '..');
const overridePath = path.join(appDir, 'src-tauri', 'tauri.autoport.json');

function probePort(port) {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.unref();
    server.once('error', reject);
    server.listen(port, '127.0.0.1', () => {
      const address = server.address();
      const resolvedPort = typeof address === 'object' && address ? address.port : port;
      server.close(() => resolve(resolvedPort));
    });
  });
}

async function findFreePort(startPort = 5173, attempts = 25) {
  for (let offset = 0; offset < attempts; offset += 1) {
    try {
      return await probePort(startPort + offset);
    } catch {
      // Try the next port.
    }
  }
  return probePort(0);
}

async function cleanup() {
  try {
    await fs.unlink(overridePath);
  } catch {
    // Ignore missing temp files.
  }
}

const port = await findFreePort();
const overrideConfig = {
  build: {
    devUrl: `http://localhost:${port}`,
    beforeDevCommand: `npx vite --port ${port} --strictPort`,
  },
};

await fs.writeFile(overridePath, `${JSON.stringify(overrideConfig, null, 2)}\n`, 'utf8');
console.log(`[tauri:autoport] Using dev port ${port}`);

const tauriBin = process.platform === 'win32' ? 'npx.cmd' : 'npx';
const child = spawn(tauriBin, ['tauri', 'dev', '--config', overridePath], {
  cwd: appDir,
  stdio: 'inherit',
  env: process.env,
});

const forwardSignal = (signal) => {
  child.kill(signal);
};

process.on('SIGINT', forwardSignal);
process.on('SIGTERM', forwardSignal);

child.on('exit', async (code, signal) => {
  process.off('SIGINT', forwardSignal);
  process.off('SIGTERM', forwardSignal);
  await cleanup();
  if (signal) {
    process.kill(process.pid, signal);
    return;
  }
  process.exit(code ?? 0);
});

child.on('error', async (error) => {
  await cleanup();
  console.error('[tauri:autoport] Failed to start Tauri dev:', error);
  process.exit(1);
});
