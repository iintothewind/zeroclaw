// Pure projection from the live message list to the blocks the chat renders.
//
// A turn watched live arrives as a run of loose pieces: the user's prompt, one
// standalone tool card per call, and finally a single assistant bubble holding
// only the final answer. Rendering that as-is reads as "one blob of text, then
// N cards", which loses the order the steps actually happened in. This module
// folds the pieces back into one turn block whose trajectory comes from
// `turnStream.logic`'s step segments.
//
// Live-only: the segments are lost on refresh, so a hydrated conversation has
// nothing to group and keeps rendering exactly as it does today. That is the
// accepted trade, asserted by tests rather than tolerated.
//
// Pure and React-free so the grouping rules are pinned without mounting a
// component, matching the repo's `.logic.ts` convention.

import type { LiveTurn, StepSegment, TurnSegments } from '@/contexts/turnStream.logic';

/** The message fields this projection reads. Structural rather than the
 *  context's `ChatMessage` so this module stays free of React. */
export interface FlowMessage {
  id: string;
  role: 'user' | 'agent';
  content: string;
  thinking?: string;
  markdown?: boolean;
  toolCall?: { name: string; args?: unknown; output?: string; id?: string };
  timestamp: Date;
  local?: boolean;
  ephemeral?: boolean;
  notice?: boolean;
  /** The turn's live trajectory, attached when the turn commits. Absent on
   *  every hydrated message — that is what keeps reloaded turns ungrouped. */
  segments?: TurnSegments;
}

export type RenderBlock =
  | { kind: 'user'; message: FlowMessage }
  | { kind: 'notice'; message: FlowMessage }
  | { kind: 'plain'; message: FlowMessage }
  | { kind: 'turn'; message: FlowMessage; segments: TurnSegments };

/** Whether a turn block is worth producing: a group with no steps would render
 *  an empty header reading `0 次工具调用`. */
function hasTrajectory(segments: TurnSegments | undefined): segments is TurnSegments {
  return Boolean(segments && segments.steps.length > 0);
}

/**
 * Group the message list into render blocks.
 *
 * A turn block absorbs the tool cards that precede its assistant message: they
 * are the same calls, now shown inside the trajectory instead of as loose
 * cards, and the group already carries their arguments and output. Tool cards
 * that are *not* followed by a trajectory (a hydrated conversation, a turn
 * still streaming, a turn that ended on a tool call) stay as their own blocks,
 * exactly as they render today.
 *
 * `showToolActivity: false` suppresses tool activity entirely, which now means
 * the group too — its whole body is tool activity. The turn's final answer
 * survives as a plain block.
 *
 * `liveSteps` is the in-flight turn's closed steps, which the transcript
 * renders as its own group below these blocks. Their calls are already in that
 * group, so the matching loose cards are dropped here rather than rendered
 * twice for the duration of the turn.
 */
export function groupMessages(
  messages: readonly FlowMessage[],
  options: {
    showToolActivity?: boolean;
    liveSteps?: readonly StepSegment[];
  } = {},
): RenderBlock[] {
  const showToolActivity = options.showToolActivity !== false;
  const blocks: RenderBlock[] = [];
  let pendingToolCards: FlowMessage[] = [];
  // Ids the in-flight group already carries. Ids are the gateway's
  // `tool_call_id`s, which the loose cards are keyed by too.
  const liveCallIds = new Set(
    (options.liveSteps ?? [])
      .flatMap((step) => step.toolCalls.map((call) => call.id))
      .filter((id): id is string => Boolean(id)),
  );

  const flushToolCards = () => {
    if (pendingToolCards.length === 0) return;
    if (showToolActivity) {
      for (const card of pendingToolCards) blocks.push({ kind: 'plain', message: card });
    }
    pendingToolCards = [];
  };

  for (const message of messages) {
    // A notice is never tool activity even if it carries a tool payload.
    if (message.role === 'agent' && message.toolCall && !message.notice) {
      const id = message.toolCall.id;
      if (id && liveCallIds.has(id)) continue;
      pendingToolCards.push(message);
      continue;
    }

    if (message.role === 'agent' && hasTrajectory(message.segments)) {
      // The trajectory supersedes the loose cards: same calls, in order.
      pendingToolCards = [];
      if (showToolActivity) {
        blocks.push({ kind: 'turn', message, segments: message.segments });
      } else {
        blocks.push({ kind: 'plain', message });
      }
      continue;
    }

    flushToolCards();
    if (message.notice) blocks.push({ kind: 'notice', message });
    else if (message.role === 'user') blocks.push({ kind: 'user', message });
    else blocks.push({ kind: 'plain', message });
  }

  flushToolCards();
  return blocks;
}

/**
 * Split the in-flight turn the way the committed one is split: everything that
 * cannot be the answer goes in the group, and the answer still arriving sits
 * below it in the bubble.
 *
 * The answer is the step being written, or — once nothing is being written —
 * the last closed step, which is the rule `finalizeSegments` applies when the
 * turn ends. Two consequences the view depends on: a tool-free turn never grows
 * a group on its way through, and text only moves once, when the turn continues
 * past it and it stops being the answer.
 */
export interface LiveTurnSplit {
  /** Steps that cannot be the answer: they belong in the group. */
  groupSteps: StepSegment[];
  /** The answer still arriving, or `null` while the model is between steps. */
  answer: StepSegment | null;
}

export function splitLiveTurn(live: LiveTurn): LiveTurnSplit {
  if (live.open.text || live.open.thinking) {
    return { groupSteps: live.steps, answer: live.open };
  }
  const last = live.steps[live.steps.length - 1];
  if (last && last.toolCalls.length === 0) {
    return { groupSteps: live.steps.slice(0, -1), answer: last };
  }
  // Nothing open and the last step made a call: the turn ended on a tool call,
  // so there is no answer text — its cards are the record.
  return { groupSteps: live.steps, answer: null };
}

/** How many tool calls a turn made — the reference header's first number. */
export function countToolCalls(segments: TurnSegments): number {
  return segments.steps.reduce((total, step) => total + step.toolCalls.length, 0);
}

/** How many messages a turn's group holds — the reference header's second
 *  number: the steps that carry thinking or text, i.e. the visible non-tool
 *  items inside the group. The reference does not document this, so if it turns
 *  out to mean something else, only this function changes. */
export function countGroupMessages(segments: TurnSegments): number {
  return segments.steps.filter((step) => step.thinking.trim() || step.text.trim()).length;
}
