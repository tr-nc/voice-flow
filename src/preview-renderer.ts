import {
  findPreviewRevisionRuns,
  matchPreviewTokens,
  samePreviewTokens,
  tokenizePreviewFrame,
  type PreviewFrame,
  type PreviewToken,
} from "./preview-model";

type RenderedToken = PreviewToken & {
  id: number;
  element: HTMLSpanElement;
  entry: HTMLSpanElement;
};

type RevisionMark = {
  element: HTMLSpanElement;
};

type CorrectionTransition = {
  start: number;
  end: number;
  replaceAfter: number;
  marks: readonly RevisionMark[];
};

export type PreviewRendererOptions = {
  animationWindow?: number;
  revisionGhostLimit?: number;
  revisionStrikeDuration?: number;
  revisionReplaceDuration?: number;
};

/** DOM implementation for PreviewFrame. It deliberately knows nothing about ASR. */
export class PreviewRenderer {
  private readonly root: HTMLElement;
  private readonly content: HTMLSpanElement;
  private readonly revisionLayer: HTMLSpanElement;
  private readonly announcement: HTMLSpanElement;
  private readonly animationWindow: number;
  private readonly revisionGhostLimit: number;
  private readonly revisionStrikeDuration: number;
  private readonly revisionReplaceDuration: number;
  private readonly reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
  private readonly supportsWebAnimations = typeof Element.prototype.animate === "function";
  private rendered: RenderedToken[] = [];
  private revisionMarks: RevisionMark[] = [];
  private nextId = 1;
  private hasRendered = false;
  private announcedText = "";

  constructor(root: HTMLElement, options: PreviewRendererOptions = {}) {
    this.root = root;
    this.animationWindow = options.animationWindow ?? 18;
    this.revisionGhostLimit = options.revisionGhostLimit ?? 12;
    this.revisionStrikeDuration = options.revisionStrikeDuration ?? 420;
    this.revisionReplaceDuration = options.revisionReplaceDuration ?? 180;
    this.content = document.createElement("span");
    this.content.className = "preview-content";
    this.content.setAttribute("aria-hidden", "true");
    this.revisionLayer = document.createElement("span");
    this.revisionLayer.className = "preview-revision-layer";
    this.revisionLayer.setAttribute("aria-hidden", "true");
    root.addEventListener("scroll", () => this.pinRevisionLayer());
    this.announcement = document.createElement("span");
    this.announcement.className = "preview-sr-text";
    this.announcement.setAttribute("aria-live", "polite");
    this.announcement.setAttribute("aria-atomic", "true");
    root.replaceChildren(this.content, this.revisionLayer, this.announcement);
    this.pinRevisionLayer();
  }

  render(frame: PreviewFrame): void {
    const leavingPrompt = !frame.prompt && this.root.classList.contains("is-prompt");
    const enteringFinal = frame.final === true && !this.root.classList.contains("is-final");
    if (leavingPrompt || enteringFinal) this.prepareCleanRender();
    this.root.classList.toggle("is-final", frame.final === true);
    this.root.classList.toggle("is-prompt", frame.prompt === true);

    const accessibleText = frame.chunks.map(({ text }) => text).join("");
    if (!accessibleText) {
      this.clear();
      return;
    }

    const nextTokens = tokenizePreviewFrame(frame);
    if (samePreviewTokens(this.rendered, nextTokens)) return;

    const matches = matchPreviewTokens(this.rendered, nextTokens);
    const animatedStart = Math.max(0, nextTokens.length - this.animationWindow);
    const oldPositions = this.measurePositions(this.rendered);
    const rootRect = this.root.getBoundingClientRect();
    const animationsEnabled = this.animationsEnabled() && !frame.final && !frame.prompt;
    this.pinRevisionLayer();

    const nextRendered = nextTokens.map((token, nextIndex) => {
      const previousIndex = matches[nextIndex];
      if (previousIndex === undefined) return this.createToken(token);

      const existing = this.rendered[previousIndex];
      existing.text = token.text;
      existing.whitespace = token.whitespace;
      existing.treatment = token.treatment;
      return existing;
    });

    const correctionInsertions = new Set<number>();
    const correctionTransitions: CorrectionTransition[] = [];
    if (animationsEnabled) {
      for (const run of findPreviewRevisionRuns(this.rendered.length, matches)) {
        const previous = this.rendered.slice(run.previousStart, run.previousEnd);
        if (!previous.some((token) => !token.whitespace)) continue;

        const transition = this.addRevisionRun(previous, oldPositions, rootRect);
        for (let index = run.nextStart; index < run.nextEnd; index += 1) {
          if (!nextRendered[index].whitespace) correctionInsertions.add(index);
        }
        correctionTransitions.push({
          start: run.nextStart,
          end: run.nextEnd,
          replaceAfter: transition.replaceAfter,
          marks: transition.marks,
        });
      }
    }

    this.reconcileContent(nextRendered);
    this.applyTreatments(nextRendered);

    if (animationsEnabled) {
      correctionTransitions.forEach(({ start, end, replaceAfter, marks }) => {
        marks.forEach((mark) => this.removeRevisionMark(mark, true, replaceAfter));
        this.animateCorrectionInsertions(nextRendered, start, end, replaceAfter);
      });
      // A correction already has its own visual transition. Moving stable
      // tokens between line boxes at the same time makes the grid feel loose.
      if (correctionTransitions.length === 0) {
        this.animateLayout(nextRendered, oldPositions, animatedStart);
      }
      this.animateInsertions(nextRendered, matches, animatedStart, correctionInsertions);
    }

    this.rendered = nextRendered;
    this.hasRendered = true;
    if (accessibleText !== this.announcedText) {
      this.announcement.textContent = accessibleText;
      this.announcedText = accessibleText;
    }
  }

  clear(): void {
    this.rendered = [];
    this.hasRendered = false;
    this.root.classList.remove("is-final");
    this.root.classList.remove("is-prompt");
    this.content.replaceChildren();
    if (this.announcedText) {
      this.announcement.textContent = "";
      this.announcedText = "";
    }
    this.revisionMarks.forEach((mark) => {
      mark.element.remove();
    });
    this.revisionMarks = [];
  }

  private prepareCleanRender(): void {
    this.rendered = [];
    this.hasRendered = false;
    this.content.replaceChildren();
    this.revisionMarks.forEach((mark) => mark.element.remove());
    this.revisionMarks = [];
  }

  private createToken(token: PreviewToken): RenderedToken {
    const element = document.createElement("span");
    const entry = document.createElement("span");
    const id = this.nextId++;
    element.className = "preview-token";
    entry.className = "preview-token__entry";
    entry.textContent = token.text;
    element.append(entry);
    return { ...token, id, element, entry };
  }

  private applyTreatments(tokens: RenderedToken[]): void {
    tokens.forEach((token) => {
      const className = [
        "preview-token",
        `preview-token--${token.treatment}`,
        token.whitespace ? "preview-token--whitespace" : "",
      ]
        .filter(Boolean)
        .join(" ");
      if (token.element.className !== className) token.element.className = className;
      if (token.entry.textContent !== token.text) token.entry.textContent = token.text;
    });
  }

  private reconcileContent(tokens: readonly RenderedToken[]): void {
    let cursor = this.content.firstChild;
    for (const { element } of tokens) {
      if (cursor === element) {
        cursor = cursor.nextSibling;
      } else {
        this.content.insertBefore(element, cursor);
      }
    }
    while (cursor) {
      const next = cursor.nextSibling;
      cursor.remove();
      cursor = next;
    }
  }

  private measurePositions(tokens: readonly RenderedToken[]): Map<number, DOMRect> {
    return new Map(tokens.map((token) => [token.id, token.element.getBoundingClientRect()]));
  }

  private animateLayout(tokens: readonly RenderedToken[], oldPositions: Map<number, DOMRect>, animatedStart: number): void {
    tokens.forEach((token, index) => {
      if (index < animatedStart || token.whitespace) return;
      const previous = oldPositions.get(token.id);
      if (!previous) return;
      const current = token.element.getBoundingClientRect();
      const x = previous.left - current.left;
      const y = previous.top - current.top;
      if (Math.abs(x) < 0.5 && Math.abs(y) < 0.5) return;
      token.element.animate(
        [{ transform: `translate(${x}px, ${y}px)` }, { transform: "translate(0, 0)" }],
        { duration: 260, easing: "cubic-bezier(.22,.75,.28,1)" },
      );
    });
  }

  private animateInsertions(
    tokens: readonly RenderedToken[],
    matches: Array<number | undefined>,
    animatedStart: number,
    correctionInsertions: ReadonlySet<number>,
  ): void {
    tokens.forEach((token, index) => {
      if (
        index < animatedStart ||
        matches[index] !== undefined ||
        token.whitespace ||
        correctionInsertions.has(index)
      ) {
        return;
      }
      token.entry.animate(
        [
          { opacity: 0, filter: "blur(2px)" },
          { opacity: 0.88, offset: 0.7 },
          { opacity: 1, filter: "blur(0)" },
        ],
        { duration: 260, easing: "cubic-bezier(.2,.78,.25,1)" },
      );
    });
  }

  private addRevisionRun(
    tokens: readonly RenderedToken[],
    positions: ReadonlyMap<number, DOMRect>,
    rootRect: DOMRect,
  ): { replaceAfter: number; marks: readonly RevisionMark[] } {
    if (this.revisionGhostLimit === 0) return { replaceAfter: 0, marks: [] };
    const firstText = tokens.findIndex((token) => !token.whitespace);
    let lastText = tokens.length - 1;
    while (lastText >= 0 && tokens[lastText].whitespace) lastText -= 1;
    if (firstText < 0 || lastText < firstText) return { replaceAfter: 0, marks: [] };
    const visibleTokens = tokens.slice(firstText, lastText + 1).filter((token) => !token.whitespace);
    const drawableLength = visibleTokens.reduce(
      (length, token) => length + Math.max(1, Array.from(token.text).length),
      0,
    );
    let strikeDelay = 0;
    const newMarks: RevisionMark[] = [];

    visibleTokens.forEach((token) => {
      const rect = positions.get(token.id);
      if (!rect || rect.bottom < rootRect.top || rect.top > rootRect.bottom) return;
      const markElement = document.createElement("span");
      markElement.className = "preview-revision-ghost";
      markElement.setAttribute("aria-hidden", "true");
      markElement.textContent = token.text;
      markElement.style.left = `${rect.left - rootRect.left}px`;
      markElement.style.top = `${rect.top - rootRect.top}px`;
      markElement.style.width = `${rect.width}px`;
      markElement.style.height = `${rect.height}px`;

      const weight = Math.max(1, Array.from(token.text).length) / drawableLength;
      const duration = Math.max(70, Math.round(this.revisionStrikeDuration * weight));
      markElement.style.setProperty("--revision-strike-delay", `${strikeDelay}ms`);
      markElement.style.setProperty("--revision-strike-duration", `${duration}ms`);
      strikeDelay += duration;

      while (this.revisionMarks.length >= this.revisionGhostLimit) {
        const oldest = this.revisionMarks[0];
        if (oldest) this.removeRevisionMark(oldest, false);
      }

      const mark: RevisionMark = {
        element: markElement,
      };
      this.revisionMarks.push(mark);
      newMarks.push(mark);
      this.revisionLayer.append(markElement);
    });
    return {
      replaceAfter: strikeDelay,
      marks: newMarks.filter((mark) => this.revisionMarks.includes(mark)),
    };
  }

  private animateCorrectionInsertions(
    tokens: readonly RenderedToken[],
    start: number,
    end: number,
    revealAfter: number,
  ): void {
    for (let index = start; index < end; index += 1) {
      const token = tokens[index];
      if (token.whitespace) continue;
      const animation = token.element.animate(
        [
          { opacity: 0, clipPath: "inset(0 100% 0 0)" },
          { opacity: 1, clipPath: "inset(0 0 0 0)" },
        ],
        {
          delay: revealAfter,
          duration: this.revisionReplaceDuration,
          easing: "cubic-bezier(.22,.75,.28,1)",
          fill: "backwards",
        },
      );
      animation.finished.catch(() => undefined);
    }
  }

  private removeRevisionMark(mark: RevisionMark, animate: boolean, delay = 0): void {
    const finish = () => {
      if (!this.revisionMarks.includes(mark)) return;
      mark.element.remove();
      this.revisionMarks = this.revisionMarks.filter((candidate) => candidate !== mark);
    };
    if (!animate || !mark.element.isConnected || !this.animationsEnabled()) {
      finish();
      return;
    }

    const animation = mark.element.animate(
      [
        { opacity: 0.68, clipPath: "inset(0 0 0 0)" },
        { opacity: 0, clipPath: "inset(0 100% 0 0)" },
      ],
      {
        delay,
        duration: this.revisionReplaceDuration,
        easing: "cubic-bezier(.22,.75,.28,1)",
        fill: "forwards",
      },
    );
    animation.finished.then(finish).catch(finish);
  }

  private pinRevisionLayer(): void {
    this.revisionLayer.style.transform = `translate(${this.root.scrollLeft}px, ${this.root.scrollTop}px)`;
  }

  private animationsEnabled(): boolean {
    return this.hasRendered && this.supportsWebAnimations && !this.reducedMotion.matches;
  }
}
