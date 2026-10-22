// Singleton Voice Note Audio Playback Manager
// Governed by PRD.md Section FR-VOICE-001 through FR-VOICE-009, TRD.md Section 4.2, and GEMINI.md

export type PlaybackState = 'idle' | 'playing' | 'paused' | 'error';

export interface AudioStateListener {
  (attachmentId: string, state: PlaybackState, currentTime: number, duration: number): void;
}

class AudioManager {
  private static instance: AudioManager;
  private currentAudio: HTMLAudioElement | null = null;
  private currentAttachmentId: string | null = null;
  private playbackRate: number = 1.0;
  private listeners: Set<AudioStateListener> = new Set();
  private updateTimer: number | null = null;

  private constructor() {}

  public static getInstance(): AudioManager {
    if (!AudioManager.instance) {
      AudioManager.instance = new AudioManager();
    }
    return AudioManager.instance;
  }

  public subscribe(listener: AudioStateListener): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  private notify(state: PlaybackState): void {
    if (!this.currentAttachmentId || !this.currentAudio) return;
    const current = this.currentAudio.currentTime || 0;
    const duration = isFinite(this.currentAudio.duration) ? this.currentAudio.duration : 0;
    for (const listener of this.listeners) {
      listener(this.currentAttachmentId, state, current, duration);
    }
  }

  public play(attachmentId: string, mediaUrl: string): void {
    // If the same audio is already playing, do nothing
    if (this.currentAttachmentId === attachmentId && this.currentAudio) {
      if (this.currentAudio.paused) {
        this.currentAudio.play().catch(() => this.notify('error'));
        this.startTimer();
        this.notify('playing');
      }
      return;
    }

    // Stop any previously playing audio
    this.stopCurrent();

    this.currentAttachmentId = attachmentId;
    this.currentAudio = new Audio(mediaUrl);
    this.currentAudio.playbackRate = this.playbackRate;

    this.currentAudio.addEventListener('loadedmetadata', () => {
      this.notify('paused');
    });

    this.currentAudio.addEventListener('play', () => {
      this.startTimer();
      this.notify('playing');
    });

    this.currentAudio.addEventListener('pause', () => {
      this.stopTimer();
      this.notify('paused');
    });

    this.currentAudio.addEventListener('ended', () => {
      this.stopTimer();
      if (this.currentAudio) {
        this.currentAudio.currentTime = 0;
      }
      this.notify('paused');
    });

    this.currentAudio.addEventListener('error', () => {
      this.stopTimer();
      this.notify('error');
    });

    this.currentAudio.play().catch(() => {
      this.notify('error');
    });
  }

  public pause(): void {
    if (this.currentAudio && !this.currentAudio.paused) {
      this.currentAudio.pause();
    }
  }

  public toggle(attachmentId: string, mediaUrl: string): void {
    if (this.currentAttachmentId === attachmentId && this.currentAudio) {
      if (this.currentAudio.paused) {
        this.currentAudio.play().catch(() => this.notify('error'));
      } else {
        this.currentAudio.pause();
      }
    } else {
      this.play(attachmentId, mediaUrl);
    }
  }

  public seek(attachmentId: string, targetSeconds: number): void {
    if (this.currentAttachmentId === attachmentId && this.currentAudio) {
      if (isFinite(targetSeconds) && targetSeconds >= 0) {
        this.currentAudio.currentTime = targetSeconds;
        this.notify(this.currentAudio.paused ? 'paused' : 'playing');
      }
    }
  }

  public setPlaybackRate(rate: number): void {
    this.playbackRate = rate;
    if (this.currentAudio) {
      this.currentAudio.playbackRate = rate;
    }
  }

  public getPlaybackRate(): number {
    return this.playbackRate;
  }

  public getCurrentAttachmentId(): string | null {
    return this.currentAttachmentId;
  }

  public isPlaying(attachmentId: string): boolean {
    return (
      this.currentAttachmentId === attachmentId &&
      this.currentAudio !== null &&
      !this.currentAudio.paused
    );
  }

  public stopCurrent(): void {
    this.stopTimer();
    if (this.currentAudio) {
      this.currentAudio.pause();
      const prevId = this.currentAttachmentId;
      this.currentAudio = null;
      this.currentAttachmentId = null;
      if (prevId) {
        for (const listener of this.listeners) {
          listener(prevId, 'idle', 0, 0);
        }
      }
    }
  }

  private startTimer(): void {
    this.stopTimer();
    this.updateTimer = window.setInterval(() => {
      if (this.currentAudio && !this.currentAudio.paused) {
        this.notify('playing');
      }
    }, 100);
  }

  private stopTimer(): void {
    if (this.updateTimer !== null) {
      clearInterval(this.updateTimer);
      this.updateTimer = null;
    }
  }
}

export const audioManager = AudioManager.getInstance();

export function formatDuration(seconds: number): string {
  if (isNaN(seconds) || !isFinite(seconds) || seconds < 0) {
    return '0:00';
  }
  const totalSecs = Math.floor(seconds);
  const mins = Math.floor(totalSecs / 60);
  const secs = totalSecs % 60;
  return `${mins}:${secs < 10 ? '0' : ''}${secs}`;
}
