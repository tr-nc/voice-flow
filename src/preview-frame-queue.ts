import type { PreviewFrame } from "./preview-model";

export type PreviewFrameDriver = {
  request(callback: () => void): number;
  cancel(frame: number): void;
};

/** Coalesces streaming preview updates without carrying a hidden-window frame across sessions. */
export class PreviewFrameQueue {
  private readonly render: (frame: PreviewFrame) => void;
  private readonly driver: PreviewFrameDriver;
  private pending: PreviewFrame | undefined;
  private scheduledFrame: number | undefined;

  constructor(render: (frame: PreviewFrame) => void, driver: PreviewFrameDriver) {
    this.render = render;
    this.driver = driver;
  }

  submit(frame: PreviewFrame): void {
    if (frame.chunks.length === 0 || frame.final || frame.prompt) {
      this.reset(frame);
      return;
    }

    this.pending = frame;
    if (this.scheduledFrame !== undefined) return;

    let requestedFrame = 0;
    requestedFrame = this.driver.request(() => {
      if (this.scheduledFrame !== requestedFrame) return;
      this.scheduledFrame = undefined;
      const pending = this.pending;
      this.pending = undefined;
      if (pending) this.render(pending);
    });
    this.scheduledFrame = requestedFrame;
  }

  private reset(frame: PreviewFrame): void {
    if (this.scheduledFrame !== undefined) {
      this.driver.cancel(this.scheduledFrame);
      this.scheduledFrame = undefined;
    }
    this.pending = undefined;
    this.render(frame);
  }
}
