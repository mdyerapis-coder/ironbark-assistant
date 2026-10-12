// Native parent launches this process inside its containment/network lease.
export {}; // module marker: worker entry uses top-level await but has no imports/exports
const environment = { PATH: process.env.PATH, LANG: process.env.LANG, LC_ALL: process.env.LC_ALL,
  HOME: process.env.IRONBARK_WORKER_HOME, TMPDIR: process.env.IRONBARK_WORKER_TMPDIR };
for (const key of Object.keys(process.env)) delete process.env[key];
for (const [key, value] of Object.entries(environment)) if (value) process.env[key] = value;

try {
  if (!process.env.HOME || !process.env.TMPDIR) throw new Error('isolated_worker_environment_required');
  // Static imports cannot enforce this security bootstrap: SDK modules snapshot
  // ambient config at evaluation time, before the environment scrub above.
  const { setTransports } = await import('@oh-my-pi/pi-utils/logger');
  setTransports({ console: false, file: false });
  const { serveProviderRpc, nativeCredentialResolver } = await import('./rpc.ts');
  const { OmpProviderAdapter } = await import('./providers/omp-sdk.ts');
  const credentials = nativeCredentialResolver(3);
  try {
    await serveProviderRpc(process.stdin, bytes => new Promise<void>((resolve, reject) => {
      process.stdout.write(bytes, error => error ? reject(error) : resolve());
    }), connection => new OmpProviderAdapter([connection], credentials));
  } finally { credentials.close(); }
} catch {
  // Exception objects can contain upstream requests and credentials. Never print them.
  process.stderr.write('ironbark provider worker failed\n');
  process.exitCode = 1;
}
