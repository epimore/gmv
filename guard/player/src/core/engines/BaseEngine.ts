import type { GmvEngine, GmvProtocol } from '../types';

export abstract class BaseEngine implements GmvEngine {
  protected video?: HTMLVideoElement;

  abstract readonly protocol: GmvProtocol;
  abstract attach(video: HTMLVideoElement, source: any): Promise<void> | void;

  play(): Promise<void> | void {
    return this.video?.play();
  }

  pause(): void {
    this.video?.pause();
  }

  abstract destroy(): void;
}
