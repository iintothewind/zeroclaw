import { t } from '@/lib/i18n';
import {
  cacheHitRatio,
  formatTokens,
  type LiveStats,
} from '@/lib/sessionStats.logic';

interface SessionStatsRowProps {
  stats: LiveStats;
}

/**
 * The composer's stats line: `本次 2 轮 10 步 · 135K tok · 缓存命中 83%`.
 *
 * Purely presentational. Every number comes from the live WebSocket window
 * (`sessionStats.logic`), so the row is small on a conversation with history
 * after a refresh — that is the designed behaviour, and the `本次` prefix is
 * what explains it. Do not "fix" it by fetching history.
 *
 * No cost segment: production models are unpriced, so a `$` figure could only
 * ever read `$0.00`.
 */
export default function SessionStatsRow({ stats }: SessionStatsRowProps) {
  const ratio = cacheHitRatio(stats);
  const segments = [
    `${t('agent.stats.session')} ${t('agent.stats.turns').replace('{count}', String(stats.turns))} ${t('agent.stats.steps').replace('{count}', String(stats.steps))}`,
    t('agent.stats.tokens').replace('{count}', formatTokens(stats.input + stats.output)),
  ];
  if (ratio !== null) {
    segments.push(
      t('agent.stats.cache_hit').replace('{percent}', String(Math.round(ratio * 100))),
    );
  }

  return (
    <div
      className="flex min-w-0 flex-1 items-center text-[11px] font-mono"
      style={{ color: 'var(--pc-text-muted)' }}
    >
      <span className="tabular-nums truncate">{segments.join(' · ')}</span>
    </div>
  );
}
