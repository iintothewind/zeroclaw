/** The trajectory vocabulary: what one LLM call produced, a turn's closed
 *  steps, and the turn still in flight.
 *
 *  Declared in a pure module because both sides of the seam read it — the
 *  reducer in `contexts/turnStream.logic` writes it, the projection in
 *  `lib/messageFlow.logic` and the group component render it. Keeping the
 *  types here is what stops `lib/` from importing `contexts/`. No React, no
 *  path aliases, so a `.logic.ts` file can import it under the Node test
 *  runner without mounting a component. */
import type { ToolCall } from './toolCall.ts';

/** One LLM call: what it thought, what it said, what it invoked. */
export interface StepSegment {
  thinking: string;
  text: string;
  /** Several calls here means one step making parallel tool calls — not
   *  several steps. */
  toolCalls: ToolCall[];
}

/** A turn's steps that are not its answer — exactly what the collapsed group
 *  renders. The step that produced the answer leaves this list and becomes the
 *  committed message's own content/thinking, so it is not duplicated here. */
export interface TurnSegments {
  steps: StepSegment[];
}

/** The turn in flight, as the streaming view needs it: the steps already closed
 *  and the step still being written.
 *
 *  The view renders the closed steps as the trajectory group and the open step
 *  as the answer still arriving — the committed layout with the answer in
 *  motion. It cannot know which step will turn out to be the final answer, so
 *  the open step sits where the answer will: below the group. */
export interface LiveTurn {
  steps: StepSegment[];
  open: StepSegment;
}

export function emptyStep(): StepSegment {
  return { thinking: '', text: '', toolCalls: [] };
}

export function emptySegments(): TurnSegments {
  return { steps: [] };
}

export function emptyLiveTurn(): LiveTurn {
  return { steps: [], open: emptyStep() };
}
