export function validateEndpoint(endpoint: string, localOnly: boolean) {
  const url = new URL(endpoint);
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash) {
    throw new Error('invalid_endpoint');
  }
  // Literal loopback only: DNS names and private LAN ranges need native network policy.
  const local = url.hostname === '127.0.0.1' || url.hostname === '[::1]';
  if (localOnly && !local) throw new Error('local_only_denied');
  if (!local && url.protocol !== 'https:') throw new Error('insecure_remote_endpoint');
  return url;
}
