// A tiny typed event emitter: views subscribe to the topics they render, so a change touches only those views.

export class Emitter<Topic extends string> {
  private map = new Map<Topic, Set<() => void>>();
  private pending = new Set<Topic>();
  private scheduled = false;

  on(topic: Topic | Topic[], fn: () => void): () => void {
    const topics = Array.isArray(topic) ? topic : [topic];
    for (const t of topics) {
      let set = this.map.get(t);
      if (!set) this.map.set(t, (set = new Set()));
      set.add(fn);
    }
    return () => { for (const t of topics) this.map.get(t)?.delete(fn); };
  }

  /** Notifies now. */
  emit(topic: Topic): void {
    for (const fn of [...(this.map.get(topic) ?? [])]) fn();
  }

  /** Notifies once per animation frame however often it is called (batches DOM work). */
  emitSoon(topic: Topic): void {
    this.pending.add(topic);
    if (this.scheduled) return;
    this.scheduled = true;
    requestAnimationFrame(() => {
      this.scheduled = false;
      const topics = [...this.pending];
      this.pending.clear();
      const called = new Set<() => void>();
      for (const t of topics) {
        for (const fn of this.map.get(t) ?? []) {
          if (called.has(fn)) continue;
          called.add(fn);
          fn();
        }
      }
    });
  }

  clear(): void {
    this.map.clear();
  }
}
