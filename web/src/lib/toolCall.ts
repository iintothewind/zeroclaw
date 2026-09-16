/** The one shape for a tool invocation, shared by every carrier that moves one
 *  around: a step segment, a persisted bubble, a live flow message, and the
 *  card component all mean exactly this. Declared in a pure module (no React,
 *  no path aliases) so any `.logic.ts` file can import it for the Node test
 *  runner without mounting a component. */
export interface ToolCall {
  name: string;
  args?: unknown;
  /** The runtime's result. `undefined` while the call is still executing;
   *  a `string` once reported. */
  output?: string;
  /** Gateway `tool_call_id` — correlates a later `tool_result` frame back to
   *  this call. */
  id?: string;
}
