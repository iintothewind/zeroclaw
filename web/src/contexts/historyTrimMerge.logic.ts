import { normalizeUserContent } from '../lib/stripServerTimestamp.ts';

/** Minimal message shape needed to merge trailing locals after a history rebuild. */
export interface TrimMergeMessage {
  role: 'user' | 'agent';
  content: string;
  local?: boolean;
  notice?: boolean;
}

/**
 * After an authoritative `getMessages` rebuild, keep only trailing in-flight
 * local user bubbles that are not already represented at the end of `rebuilt`.
 *
 * Matching uses normalized content (server timestamp envelopes stripped) and
 * tail-aligns so duplicate identical sends are not over-deduped. Id/timestamp
 * equality is intentionally unused: server rows get fresh UUIDs and synthetic
 * timestamps in `mapServerMessagesToPersisted`.
 */
export function selectLocalPendingAfterRebuild<T extends TrimMergeMessage>(
  prev: T[],
  rebuilt: T[],
): T[] {
  let lastServerIdx = -1;
  for (let i = prev.length - 1; i >= 0; i--) {
    // Avoid shadowing outer WS `msg` binders when this runs inside AgentContext.
    const m = prev[i];
    if (m && !m.local && !m.notice) {
      lastServerIdx = i;
      break;
    }
  }
  const trailing = prev.slice(lastServerIdx + 1);
  const trailingLocal = trailing.filter((m) => m.local === true && m.role === 'user');
  const rebuiltUsers = rebuilt.filter((r) => r.role === 'user');

  let i = trailingLocal.length - 1;
  let j = rebuiltUsers.length - 1;
  while (i >= 0 && j >= 0) {
    const local = trailingLocal[i];
    const server = rebuiltUsers[j];
    if (
      local
      && server
      && normalizeUserContent(local.content) === normalizeUserContent(server.content)
    ) {
      i -= 1;
      j -= 1;
    } else {
      break;
    }
  }
  return trailingLocal.slice(0, i + 1);
}
