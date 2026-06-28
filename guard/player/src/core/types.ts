export type GmvProtocol = 'flv' | 'fmp4' | 'hls';
export type GmvCodec = 'h264' | 'h265';
export type GmvDeviceStatus = 'online' | 'offline' | 'playing' | 'reconnecting' | 'error' | 'idle';

export interface GmvSource {
  protocol: GmvProtocol;
  url: string;
  codec?: GmvCodec;
  mimeCodec?: string;
  hasAudio?: boolean;
  priority?: number;
  label?: string;
}

export interface GmvPlayerCoreOptions {
  video: HTMLVideoElement;
  sources: GmvSource[];
  autoplay?: boolean;
  muted?: boolean;
  lowLatency?: boolean;
  fallback?: boolean;
  reconnect?: {
    maxRetries?: number;
    baseDelayMs?: number;
  };
}

export interface GmvEngine {
  readonly protocol: GmvProtocol;
  attach(video: HTMLVideoElement, source: GmvSource): Promise<void> | void;
  play(): Promise<void> | void;
  pause(): void;
  destroy(): void;
}

export type GmvEngineFactory = () => GmvEngine;

export interface GmvPlayerEvents {
  loading: undefined;
  playing: undefined;
  paused: undefined;
  stalled: undefined;
  reconnecting: { retry: number; reason: string };
  reconnected: undefined;
  error: { code: string; message: string; source?: GmvSource };
  sourceChanged: { source: GmvSource };
  stats: {
    protocol: GmvProtocol;
    codec?: GmvCodec;
    bitrate?: number;
    fps?: number;
    bufferSeconds?: number;
    viewers?: number;
  };
  destroyed: undefined;
}

export type GmvPlayerEvent = keyof GmvPlayerEvents;

export interface GmvOsdItem {
  id: string;
  text: string;
  x: number;
  y: number;
}

export interface GmvAiBox {
  id: string;
  label: string;
  confidence?: number;
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface GmvViewCapabilities {
  ptz?: boolean;
  presets?: boolean;
  snapshot?: boolean;
  record?: boolean;
  playback?: boolean;
  talk?: boolean;
  streamSwitch?: boolean;
  aiOverlay?: boolean;
}

export interface GmvPtzCommand {
  action:
    | 'up'
    | 'down'
    | 'left'
    | 'right'
    | 'leftUp'
    | 'rightUp'
    | 'leftDown'
    | 'rightDown'
    | 'zoomIn'
    | 'zoomOut'
    | 'focusNear'
    | 'focusFar'
    | 'irisOpen'
    | 'irisClose'
    | 'stop';
  speed: number;
}
