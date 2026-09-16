// Pure completion/turn-stream reducer used by AgentContext's WebSocket handler
// so the production lifecycle is testable without mounting React. This file
// makes no DOM/i18n/UUID side effects; it owns only per-turn stream state,
// completion classification, and the turn's step trajectory.
//
// Two views are folded from the same frames:
//   - the flat buffers (`pendingContent` / `pendingThinking`) that
//     `classifyCompletion` needs, kept byte-for-byte as they were;
//   - the ordered step segments that the message-flow view renders.
// The second is strictly additive: `classifyCompletion` must keep behaving
// identically, and its existing tests pin that.

/** A tool invocation as it appears inside a step. Structurally identical to
 *  `ToolCallInfo` from `ToolCallCard`; declared here so this module stays free
 *  of component imports (the Node test runner loads it directly). */
export interface SegmentToolCall {
  name: string;
  args?: unknown;
  output?: string;
  /** Gateway `tool_call_id` — correlates a later result to this call. */
  id?: string;
}

/** One LLM call: what it thought, what it said, what it invoked. */
export interface StepSegment {
  thinking: string;
  text: string;
  /** Several calls here means one step making parallel tool calls — not
   *  several steps. */
  toolCalls: SegmentToolCall[];
}

/** A turn's full trajectory, in order. */
export interface TurnSegments {
  /** Closed steps, in the order they happened. The step that produced the
   *  final answer is not here: it becomes `finalText` / `finalThinking`. */
  steps: StepSegment[];
  /** The turn's final answer, rendered outside the collapsed group. */
  finalText: string;
  /** Reasoning of the step that produced the final answer. */
  finalThinking: string;
}

/** The slice of turn state `classifyCompletion` reads. Declared on its own so a
 *  caller can classify a finished turn without materialising a full
 *  `TurnStreamState` (the trajectory is irrelevant to the decision). */
export interface CompletionInput {
  pendingContent: string;
  pendingThinking: string;
  capturedThinking: string;
  hadToolCall: boolean;
}

/** Accumulated per-turn streaming state the reducer folds frames into. */
export interface TurnStreamState extends CompletionInput {
  /** The turn's trajectory so far. */
  segments: TurnSegments;
  /** The step currently being written. `thinking` / `chunk` land here until the
   *  `usage` frame closes it. */
  openStep: StepSegment;
}

export function emptyStep(): StepSegment {
  return { thinking: '', text: '', toolCalls: [] };
}

export function emptySegments(): TurnSegments {
  return { steps: [], finalText: '', finalThinking: '' };
}

/** Fresh per-turn state. Returned after every completion so the next turn
 *  starts clean — the "ref state resets across turns" invariant. */
export function initialTurnStreamState(): TurnStreamState {
  return {
    pendingContent: '',
    pendingThinking: '',
    capturedThinking: '',
    hadToolCall: false,
    segments: emptySegments(),
    openStep: emptyStep(),
  };
}

/** Events that affect the current turn's accumulated stream state. The handler
 *  maps gateway frames and local lifecycle boundaries to these before folding;
 *  events the completion decision does not depend on are not modeled. */
export type TurnStreamFrame =
  | { type: 'thinking'; content?: string }
  | { type: 'chunk'; content?: string }
  | { type: 'chunk_reset' }
  // `hasName` mirrors the handler's `if (!msg.name) break` guard: a nameless
  // observability telemetry frame is not a real tool call and must not flip
  // `hadToolCall` (issue #7151). `call` carries the payload for the step
  // trajectory and is absent on such frames.
  | { type: 'tool_call'; hasName: boolean; call?: SegmentToolCall }
  | { type: 'tool_result'; id?: string; output: string }
  // One LLM call completed. This is the *only* server-authoritative step
  // boundary: it arrives once the response is accepted, before that step's
  // tool-call events.
  | { type: 'usage' }
  | { type: 'done' | 'message'; full_response?: string; content?: string }
  // Every terminal/non-completion boundary resets the same canonical state.
  // `turn_start` is emitted locally after a new message is sent.
  | { type: 'turn_start' | 'aborted' | 'error' | 'reset' };

/** What the handler should do with the finished turn. `commit` renders an
 *  assistant bubble (possibly empty content when reasoning is present);
 *  `diagnostic` renders the one-off no-output notice; `skip` renders nothing
 *  (the tool cards are already the visible record — #6702). */
export type CompletionOutcome =
  | { kind: 'commit'; content: string; thinking?: string }
  | { kind: 'diagnostic' }
  | { kind: 'skip' };

/** Classify a finished turn from its accumulated state and the terminal frame.
 *  Pure: the whole reasoning-only / empty / tool-only decision lives here. */
export function classifyCompletion(
  state: CompletionInput,
  frame: { full_response?: string; content?: string },
): CompletionOutcome {
  // Fallback chain matches the handler: an explicit `full_response`, then a
  // frame `content`, then whatever was streamed live.
  const raw = frame.full_response ?? frame.content ?? state.pendingContent;
  // Trim so whitespace-only content (models that emit "\n\n" alongside
  // tool_calls) does not create a blank bubble (#6702).
  const content = raw.trim();
  const thinking =
    state.capturedThinking || state.pendingThinking || undefined;

  if (content || thinking) {
    // Reasoning-only turns land here with empty content but present thinking,
    // so the turn is visible instead of vanishing silently.
    return { kind: 'commit', content, thinking };
  }
  if (!state.hadToolCall) {
    // Nothing at all on a clean completion — surface a diagnostic so the turn
    // does not disappear. Mirrors zerocode's zc-turn-no-output (#8779).
    return { kind: 'diagnostic' };
  }
  // Empty content but tools ran: their cards are the record, render nothing.
  return { kind: 'skip' };
}

function stepHasContent(step: StepSegment): boolean {
  return Boolean(step.thinking || step.text || step.toolCalls.length);
}

/** Close the step being written, pushing it onto the trajectory only if it
 *  carries something. An empty step is not a step. */
export function closeStep(state: TurnStreamState): TurnStreamState {
  if (!stepHasContent(state.openStep)) return state;
  return {
    ...state,
    segments: {
      ...state.segments,
      steps: [...state.segments.steps, state.openStep],
    },
    openStep: emptyStep(),
  };
}

/** Attach a tool call to the step that made it.
 *
 *  The `usage` frame closes a step as soon as its response is accepted, and
 *  the tool-call events for that step arrive *after* it — so a call lands on
 *  the last closed step, and several calls landing there is parallel tool use
 *  within one step. A call with no step to attach to (no preceding thinking or
 *  text) opens a step of its own, because the tools are the record. */
export function attachToolCall(
  state: TurnStreamState,
  call: SegmentToolCall,
): TurnStreamState {
  const closed = closeStep(state);
  const steps =
    closed.segments.steps.length > 0
      ? closed.segments.steps
      : [emptyStep()];
  const last = steps[steps.length - 1]!;
  const nextSteps = [
    ...steps.slice(0, -1),
    { ...last, toolCalls: [...last.toolCalls, call] },
  ];
  return { ...closed, segments: { ...closed.segments, steps: nextSteps } };
}

/** Locate the call a result belongs to. Correlates by gateway `tool_call_id`
 *  so out-of-order parallel results land on the right call; falls back to the
 *  first unresolved call so id-less streams still resolve. Mirrors
 *  `resolveToolResultIndex`. */
function findCallIndex(
  steps: readonly StepSegment[],
  resultId: string | undefined,
): { stepIndex: number; callIndex: number } | null {
  const firstUnresolved = (): { stepIndex: number; callIndex: number } | null => {
    for (let s = 0; s < steps.length; s++) {
      const c = steps[s]!.toolCalls.findIndex((call) => call.output === undefined);
      if (c !== -1) return { stepIndex: s, callIndex: c };
    }
    return null;
  };
  if (!resultId) return firstUnresolved();
  for (let s = 0; s < steps.length; s++) {
    const c = steps[s]!.toolCalls.findIndex(
      (call) => call.output === undefined && call.id === resultId,
    );
    if (c !== -1) return { stepIndex: s, callIndex: c };
  }
  return firstUnresolved();
}

/** Fill the matching call's output by `tool_call_id`. */
export function attachToolResult(
  state: TurnStreamState,
  id: string | undefined,
  output: string,
): TurnStreamState {
  const found = findCallIndex(state.segments.steps, id);
  if (!found) return state;
  const { stepIndex, callIndex } = found;
  const step = state.segments.steps[stepIndex]!;
  const toolCalls = [...step.toolCalls];
  toolCalls[callIndex] = { ...toolCalls[callIndex]!, output };
  const steps = [...state.segments.steps];
  steps[stepIndex] = { ...step, toolCalls };
  return { ...state, segments: { ...state.segments, steps } };
}

/** Turn the accumulated trajectory into the finished shape: the step that
 *  produced the final answer leaves `steps` and becomes the answer itself.
 *
 *  Only a step with no tool calls can be the final answer — a turn that ends
 *  on a tool call has no answer text, and its step stays in the trajectory. */
function finalizeSegments(
  state: TurnStreamState,
  frame: { full_response?: string; content?: string },
): TurnSegments {
  const closed = closeStep(state);
  const steps = [...closed.segments.steps];
  let finalThinking = closed.capturedThinking || closed.pendingThinking || '';
  const last = steps[steps.length - 1];
  if (last && last.toolCalls.length === 0) {
    steps.pop();
    finalThinking = last.thinking || finalThinking;
  }
  return {
    steps,
    finalText: (frame.full_response ?? frame.content ?? closed.pendingContent).trim(),
    finalThinking,
  };
}

/** Fold one frame into the turn state. For `done`/`message` the returned
 *  `completion` is the classification and the returned `state` is fresh (the
 *  turn is over); for every other frame `completion` is null and `state`
 *  carries the accumulation forward.
 *
 *  `result.segments` is the trajectory as of *after* the fold: for a terminal
 *  frame it is the finished shape (read it before the state resets), and for
 *  every other frame it is the trajectory so far. */
export function reduceTurnFrame(
  state: TurnStreamState,
  frame: TurnStreamFrame,
): { state: TurnStreamState; completion: CompletionOutcome | null; segments: TurnSegments } {
  switch (frame.type) {
    case 'thinking':
      return {
        state: {
          ...state,
          pendingThinking: state.pendingThinking + (frame.content ?? ''),
          openStep: { ...state.openStep, thinking: state.openStep.thinking + (frame.content ?? '') },
        },
        completion: null,
        segments: state.segments,
      };
    case 'chunk':
      return {
        state: {
          ...state,
          pendingContent: state.pendingContent + (frame.content ?? ''),
          openStep: { ...state.openStep, text: state.openStep.text + (frame.content ?? '') },
        },
        completion: null,
        segments: state.segments,
      };
    case 'chunk_reset':
      // Snapshot thinking, then clear the live display buffers. The server
      // signals the authoritative done message follows. The trajectory is
      // unaffected: it is the record, not the display.
      return {
        state: {
          ...state,
          capturedThinking: state.pendingThinking,
          pendingContent: '',
          pendingThinking: '',
        },
        completion: null,
        segments: state.segments,
      };
    case 'usage': {
      const closed = closeStep(state);
      return { state: closed, completion: null, segments: closed.segments };
    }
    case 'tool_call': {
      if (!frame.hasName) return { state, completion: null, segments: state.segments };
      const withCall = frame.call ? attachToolCall(state, frame.call) : state;
      return {
        state: { ...withCall, hadToolCall: true },
        completion: null,
        segments: withCall.segments,
      };
    }
    case 'tool_result': {
      const resolved = attachToolResult(state, frame.id, frame.output);
      return { state: resolved, completion: null, segments: resolved.segments };
    }
    case 'done':
    case 'message': {
      const completion = classifyCompletion(state, frame);
      const segments = finalizeSegments(state, frame);
      // Turn is over: hand back fresh state so the next turn starts clean.
      return { state: initialTurnStreamState(), completion, segments };
    }
    case 'aborted':
    case 'error':
    case 'turn_start':
    case 'reset': {
      // Keep whatever was accumulated for the caller to inspect, but the next
      // turn starts clean.
      const segments = finalizeSegments(state, {});
      return { state: initialTurnStreamState(), completion: null, segments };
    }
  }
}
