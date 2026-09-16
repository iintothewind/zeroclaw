import { ChevronDown, ChevronRight } from 'lucide-react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { t } from '@/lib/i18n';
import ToolCallCard from '@/components/ToolCallCard';
import type { TurnSegments } from '@/lib/turnSegments';
import { countGroupMessages, countToolCalls } from '@/lib/messageFlow.logic';

interface ToolCallGroupProps {
  segments: TurnSegments;
  compact: boolean;
  collapsed: boolean;
  onToggle: () => void;
}

/**
 * A turn's non-final steps, collapsed under a summary header reading
 * `7 次工具调用 · 6 条消息`, expanding into the full trajectory in order:
 * each step's thinking, then its text, then its tool cards.
 *
 * The final answer is never inside this group — it renders below, expanded,
 * which is what makes the reference's layout possible.
 *
 * Reuses `ToolCallCard` unchanged and the existing `<details>` pattern for
 * thinking, so a card behaves identically in and out of a group.
 */
export default function ToolCallGroup({
  segments,
  compact,
  collapsed,
  onToggle,
}: ToolCallGroupProps) {
  const calls = countToolCalls(segments);
  const messages = countGroupMessages(segments);
  const summary = t('agentchat.tool_group_summary')
    .replace('{calls}', String(calls))
    .replace('{messages}', String(messages));

  return (
    <div className="mb-1.5 min-w-0">
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={!collapsed}
        aria-label={t(collapsed ? 'agentchat.tool_group_expand' : 'agentchat.tool_group_collapse')}
        className="flex w-full items-center gap-1.5 rounded-[var(--radius-md)] border border-pc-border bg-pc-code px-2 py-1 text-left text-xs transition-colors hover:border-pc-accent/40"
        style={{ color: 'var(--pc-text-muted)' }}
      >
        {collapsed ? (
          <ChevronRight className="h-3.5 w-3.5 shrink-0" aria-hidden />
        ) : (
          <ChevronDown className="h-3.5 w-3.5 shrink-0" aria-hidden />
        )}
        <span className="truncate tabular-nums">{summary}</span>
      </button>

      {!collapsed && (
        <div className="mt-1.5 flex flex-col gap-1.5">
          {segments.steps.map((step, stepIdx) => (
            <div key={stepIdx} className="flex flex-col gap-1.5 min-w-0">
              {step.thinking && (
                <details className="min-w-0">
                  <summary className="cursor-pointer select-none text-xs text-pc-text-muted hover:text-pc-text-secondary">
                    {t('agentchat.thinking')}
                  </summary>
                  <pre className="mt-1 max-h-60 overflow-auto whitespace-pre-wrap break-words rounded-[var(--radius-sm)] bg-pc-code p-1.5 text-xs leading-snug text-pc-text-muted">
                    {step.thinking}
                  </pre>
                </details>
              )}
              {step.text && (
                <div
                  className={`${compact ? 'text-xs' : 'text-sm'} break-words leading-snug chat-markdown chat-markdown-dense`}
                >
                  <ReactMarkdown remarkPlugins={[remarkGfm]}>{step.text}</ReactMarkdown>
                </div>
              )}
              {step.toolCalls.map((call, callIdx) => (
                <ToolCallCard key={call.id ?? `${stepIdx}-${callIdx}`} toolCall={call} />
              ))}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
