export type GmvProtocol = 'flv' | 'fmp4' | 'hls' | 'mp4';
export type GmvCodec = 'h264' | 'h265';
export type GmvMediaMode = 'live' | 'playback';
export type GmvPlaybackRateMode = 'local-file' | 'remote-stream' | 'disabled';
export type GmvDeviceStatus = 'online' | 'offline' | 'playing' | 'reconnecting' | 'error' | 'idle';

export interface GmvSource {
  protocol: GmvProtocol;
  url: string;
  codec?: GmvCodec;
  mimeCodec?: string;
  hasAudio?: boolean;
  priority?: number;
  label?: string;
  rateMode?: GmvPlaybackRateMode;
  startupTimeoutMs?: number;
}

export interface GmvPlayerCoreOptions {
  video: HTMLVideoElement;
  sources: GmvSource[];
  autoplay?: boolean;
  muted?: boolean;
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
  recoverMediaError?(): boolean;
  destroy(): void;
}

export type GmvEngineFactory = () => GmvEngine;

export interface GmvPlayerEvents {
  loading: undefined;
  playing: undefined;
  paused: undefined;
  stalled: undefined;
  reconnecting: { retry: number; reason: string };
  startupProgress: { stage: 'buffering_player'; elapsedMs: number; checkpointMs: number; hardTimeoutMs: number };
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
  audio?: boolean;
  talk?: boolean;
  streamSwitch?: boolean;
  aiOverlay?: boolean;
}

export type GmvPlayerControl =
  | 'play'
  | 'audio'
  | 'snapshot'
  | 'outputType'
  | 'info'
  | 'fullscreen'
  | 'ptz'
  | 'record'
  | 'cloudRecord'
  | 'talk'
  | 'streamSwitch'
  | 'playbackRate'
  | 'playbackClip'
  | 'timeline'
  | 'presets';

export type GmvControlsVisibility = 'auto' | 'always' | 'hidden';

export interface GmvPlayerControlsConfig {
  items: GmvPlayerControl[];
  overflowItems?: GmvPlayerControl[];
  visibility?: GmvControlsVisibility;
  autoHideDelayMs?: number;
  playbackRates?: number[];
}

export interface GmvCloudRecordRange {
  startTimeMs: number;
  endTimeMs: number;
}

export interface GmvPlayerControlsState {
  playbackState: GmvDeviceStatus;
  audioEnabled: boolean;
  fullscreen: boolean;
  infoOpen: boolean;
  ptzOpen: boolean;
  recording: boolean;
  talking: boolean;
  playbackRate: number;
  seekMs: number;
  durationMs: number;
  timelineStartTimeMs?: number;
  timelineEndTimeMs?: number;
  cloudRecordLockedRange?: GmvCloudRecordRange;
  selectedSourceUrl: string;
  selectedOutputType: string;
}

export interface GmvPlayerOutputOption {
  value: string;
  label: string;
}

export type GmvPlayerControlAction =
  | { type: 'play-toggle' }
  | { type: 'audio-toggle' }
  | { type: 'snapshot' }
  | { type: 'output-type-change'; outputType: string }
  | { type: 'info-toggle' }
  | { type: 'fullscreen-toggle' }
  | { type: 'ptz-toggle' }
  | { type: 'record-toggle' }
  | { type: 'cloud-record-request' }
  | { type: 'talk-toggle' }
  | { type: 'stream-switch'; sourceUrl: string }
  | { type: 'rate-change'; rate: number }
  | { type: 'cloud-record-create'; startTimeMs: number; endTimeMs: number }
  | { type: 'seek'; timeMs: number }
  | { type: 'preset-call'; presetId: string }
  | { type: 'preset-set'; presetId: string };

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
