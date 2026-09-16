import { t } from '@/lib/i18n';
import {
  cacheHitRatio,
  formatTokens,
  type LiveStats,
} from '@/lib/sessionStats.logic';

interface SessionStatsRowProps {
  stats: LiveStats;
  compact?: boolean;
}

/** Renders `N turns · M steps · … tok · cache hit …%`. Semantics:
 *  `docs/reports/webui-overall-plan.md` §10 "Composer stats semantics". */
export default function SessionStatsRow({ stats, compact = false }: SessionStatsRowProps) {
  const segments = [
    `${t('agent.stats.turns').replace('{count}', String(stats.turns))} ${t('agent.stats.steps').replace('{count}', String(stats.steps))}`,
    t('agent.stats.tokens').replace('{count}', formatTokens(stats.input + stats.output)),
    t('agent.stats.cache_hit').replace(
      '{percent}',
      String(Math.round(cacheHitRatio(stats) * 100)),
    ),
  ];

  return (
    <div
      className={compact
        ? 'min-w-0 overflow-hidden text-[11px] font-mono leading-none'
        : 'flex min-w-0 flex-1 items-center text-[11px] font-mono'}
      style={{ color: 'var(--pc-text-muted)' }}
    >
      <span className="tabular-nums truncate block">{segments.join(' · ')}</span>
    </div>
  );
}
