/**
 * Strip gateway/runtime timestamp envelopes from server-sourced user content.
 *
 * Two shapes appear in history:
 * - Channel wall-clock: `[YYYY-MM-DD HH:MM:SS TZ] …`
 * - Agent enrich: `[CURRENT DATE & TIME: YYYY-MM-DD HH:MM:SS TZ]\n\n…`
 *
 * Anchored at the start so a bracketed datetime mid-message is left intact.
 * The zone is a chrono `%Z` abbreviation that JS `Date` can't reliably parse,
 * so we only strip for display / content comparison.
 */

/** Channel-style prefix from `timestamp_channel_user_content`. */
const CHANNEL_TIMESTAMP_RE = /^\s*\[\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2} [^\]]+\]\s*/;

const AGENT_LABEL = '[CURRENT DATE & TIME:';
const AGENT_INNER_TS_RE = /^\d{4}-\d{2}-\d{2}\s+\d{2}:\d{2}:\d{2}\s+\S+$/;

function stripAgentCurrentDateEnvelope(content: string): string {
  const trimmedStart = content.replace(/^\s+/, '');
  if (!trimmedStart.startsWith(AGENT_LABEL)) {
    return content;
  }
  const afterLabel = trimmedStart.slice(AGENT_LABEL.length);
  const bracketEnd = afterLabel.indexOf(']');
  if (bracketEnd < 0) {
    return content;
  }
  const timestamp = afterLabel.slice(0, bracketEnd).trim();
  if (!AGENT_INNER_TS_RE.test(timestamp)) {
    return content;
  }
  let rest = afterLabel.slice(bracketEnd + 1);
  if (rest.startsWith('\r\n\r\n')) {
    rest = rest.slice(4);
  } else if (rest.startsWith('\n\n')) {
    rest = rest.slice(2);
  } else {
    // Labeled envelope without the expected blank line — treat as non-match
    // so a user-authored example starting with the label is preserved.
    return content;
  }
  return rest;
}

/** Remove a leading channel or agent timestamp envelope, if present. */
export function stripServerTimestamp(content: string): string {
  const withoutAgent = stripAgentCurrentDateEnvelope(content);
  if (withoutAgent !== content) {
    return withoutAgent;
  }
  return content.replace(CHANNEL_TIMESTAMP_RE, '');
}

/** Normalize user bubble text for equality checks across local vs server forms. */
export function normalizeUserContent(content: string): string {
  return stripServerTimestamp(content).trim();
}
