// Events between a project window and its presentation windows (kept apart so the project window does not load
// the presentation chunk until a presentation starts).
export const PRESENT_EVENT = {
  /** controller → presenter window */ state: "present:state",
  /** presenter window → controller */ action: "present:action", hello: "present:hello",
  /** project → controller */ reload: "present:reload", end: "present:end", focus: "present:focus",
  /** controller → project */ ended: "present:ended", failed: "present:failed",
} as const;
