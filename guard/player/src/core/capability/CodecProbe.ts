export class CodecProbe {
  static async canDecode(contentType: string): Promise<boolean> {
    if (!('mediaCapabilities' in navigator)) {
      return typeof MediaSource !== 'undefined' && MediaSource.isTypeSupported(contentType);
    }

    try {
      const result = await navigator.mediaCapabilities.decodingInfo({
        type: 'media-source',
        video: {
          contentType,
          width: 1920,
          height: 1080,
          bitrate: 1024_000,
          framerate: 25,
        },
      });
      return result.supported === true;
    } catch {
      return typeof MediaSource !== 'undefined' && MediaSource.isTypeSupported(contentType);
    }
  }
}
