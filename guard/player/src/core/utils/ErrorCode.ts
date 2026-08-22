export const GmvErrorCode = {
  NoSource: 'NO_SOURCE',
  UnsupportedProtocol: 'UNSUPPORTED_PROTOCOL',
  UnsupportedCodec: 'UNSUPPORTED_CODEC',
  CodecMetadataMissing: 'CODEC_METADATA_MISSING',
  EngineLoadFailed: 'ENGINE_LOAD_FAILED',
  StreamOpenFailed: 'STREAM_OPEN_FAILED',
  StreamReadFailed: 'STREAM_READ_FAILED',
  MediaSourceUnavailable: 'MEDIA_SOURCE_UNAVAILABLE',
  SourceBufferFailed: 'SOURCE_BUFFER_FAILED',
} as const;
