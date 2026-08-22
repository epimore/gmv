export type EventHandler<T> = (payload: T) => void;

export class EventBus<Events extends object> {
  private readonly handlers = new Map<keyof Events, Set<EventHandler<any>>>();

  on<K extends keyof Events>(event: K, handler: EventHandler<Events[K]>): () => void {
    const handlers = this.handlers.get(event) ?? new Set<EventHandler<Events[K]>>();
    handlers.add(handler);
    this.handlers.set(event, handlers as Set<EventHandler<any>>);
    return () => this.off(event, handler);
  }

  off<K extends keyof Events>(event: K, handler: EventHandler<Events[K]>): void {
    this.handlers.get(event)?.delete(handler);
  }

  emit<K extends keyof Events>(event: K, payload: Events[K]): void {
    this.handlers.get(event)?.forEach((handler) => handler(payload));
  }

  clear(): void {
    this.handlers.clear();
  }
}
