/** Unreal `ETraceFrameType` values from MiscTrace.h. */
export const TRACE_FRAME_TYPE_GAME = 0;
export const TRACE_FRAME_TYPE_RENDERING = 1;

export type TraceFrameTypeFilter = "game" | "rendering" | "all";

export function frameTypeLabel(frameType: number): string {
  switch (frameType) {
    case TRACE_FRAME_TYPE_GAME:
      return "Game";
    case TRACE_FRAME_TYPE_RENDERING:
      return "Rendering";
    default:
      return `Type ${frameType}`;
  }
}

/**
 * Insights Frames track is one series per `ETraceFrameType`. Mixing Game and
 * Rendering in a single chart makes hitch comparison against Insights misleading.
 */
export function filterFramesByType<T extends { frame_type: number }>(
  frames: readonly T[],
  filter: TraceFrameTypeFilter,
): T[] {
  switch (filter) {
    case "game":
      return frames.filter((frame) => frame.frame_type === TRACE_FRAME_TYPE_GAME);
    case "rendering":
      return frames.filter(
        (frame) => frame.frame_type === TRACE_FRAME_TYPE_RENDERING,
      );
    case "all":
      return [...frames];
  }
}
