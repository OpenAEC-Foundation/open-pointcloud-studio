/**
 * Render scheduling for the on-demand draw loop.
 *
 * The viewer only draws a frame when something changed. Any module that
 * mutates the scene — the LOD controller streaming in nodes, a parser
 * finishing, a material uniform changing — calls requestRender() so the
 * change actually reaches the screen.
 *
 * This lives in its own module so engine code can request frames without
 * importing the React component that owns the loop (which would be a cycle).
 */

let requester: (() => void) | null = null;

/** Called by the viewer to register its loop. Pass null on teardown. */
export function setRenderRequester(fn: (() => void) | null): void {
  requester = fn;
}

/** Ask for one more frame. A no-op when no viewer is mounted. */
export function requestRender(): void {
  requester?.();
}
