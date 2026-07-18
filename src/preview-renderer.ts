import {
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

export type PreviewRendererOptions = {
  animationWindow?: number;
  revisionGhostLimit?: number;
};

/** DOM implementation for PreviewFrame. It deliberately knows nothing about ASR. */
export class PreviewRenderer {
  private readonly content: HTMLSpanElement;
  private readonly announcement: HTMLSpanElement;
  private readonly animationWindow: number;
  private readonly revisionGhostLimit: number;
  private readonly reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
  private readonly supportsWebAnimations = typeof Element.prototype.animate === "function";
  private rendered: RenderedToken[] = [];
  private revisionGhosts: HTMLSpanElement[] = [];
  private nextId = 1;
  private hasRendered = false;
  private announcedText = "";

  constructor(private readonly root: HTMLElement, options: PreviewRendererOptions = {}) {
    this.animationWindow = options.animationWindow ?? 18;
    this.revisionGhostLimit = options.revisionGhostLimit ?? 12;
    this.content = document.createElement("span");
    this.content.className = "preview-content";
    this.content.setAttribute("aria-hidden", "true");
    this.announcement = document.createElement("span");
    this.announcement.className = "preview-sr-text";
    this.announcement.setAttribute("aria-live", "polite");
    this.announcement.setAttribute("aria-atomic", "true");
    root.replaceChildren(this.content, this.announcement);
  }

  render(frame: PreviewFrame): void {
    const accessibleText = frame.chunks.map(({ text }) => text).join("");
    if (!accessibleText) {
      this.clear();
      return;
    }

    const nextTokens = tokenizePreviewFrame(frame);
    if (samePreviewTokens(this.rendered, nextTokens)) return;

    const matches = matchPreviewTokens(this.rendered, nextTokens);
    const matchedPrevious = new Set(matches.filter((index): index is number => index !== undefined));
    const animatedStart = Math.max(0, nextTokens.length - this.animationWindow);
    const oldPositions = this.measurePositions(this.rendered);
    const rootRect = this.root.getBoundingClientRect();

    if (this.animationsEnabled()) {
      for (let index = 0; index < this.rendered.length; index += 1) {
        if (!matchedPrevious.has(index) && index >= this.rendered.length - this.animationWindow) {
          this.addRevisionGhost(this.rendered[index], rootRect);
        }
      }
    }

    const nextRendered = nextTokens.map((token, nextIndex) => {
      const previousIndex = matches[nextIndex];
      if (previousIndex === undefined) return this.createToken(token);

      const existing = this.rendered[previousIndex];
      existing.text = token.text;
      existing.whitespace = token.whitespace;
      existing.treatment = token.treatment;
      return existing;
    });

    this.reconcileContent(nextRendered);
    this.applyTreatments(nextRendered);

    if (this.animationsEnabled()) {
      this.animateLayout(nextRendered, oldPositions, animatedStart);
      this.animateInsertions(nextRendered, matches, animatedStart);
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
    this.content.replaceChildren();
    if (this.announcedText) {
      this.announcement.textContent = "";
      this.announcedText = "";
    }
    this.revisionGhosts.forEach((ghost) => ghost.remove());
    this.revisionGhosts = [];
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
    tokens.forEach((token, index) => {
      const processing = token.treatment === "processing";
      const previous = tokens[index - 1];
      const next = tokens[index + 1];
      const processingStart =
        processing && (!previous || previous.treatment !== "processing" || previous.text.includes("\n"));
      const processingEnd =
        processing &&
        (!next || next.treatment !== "processing" || token.text.includes("\n") || next.text.includes("\n"));
      const className = [
        "preview-token",
        `preview-token--${token.treatment}`,
        token.whitespace ? "preview-token--whitespace" : "",
        processingStart ? "preview-token--processing-start" : "",
        processingEnd ? "preview-token--processing-end" : "",
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
  ): void {
    tokens.forEach((token, index) => {
      if (index < animatedStart || matches[index] !== undefined || token.whitespace) return;
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

  private addRevisionGhost(token: RenderedToken, rootRect: DOMRect): void {
    if (token.whitespace || this.revisionGhostLimit === 0) return;
    const rect = token.element.getBoundingClientRect();
    if (rect.bottom < rootRect.top || rect.top > rootRect.bottom) return;

    const ghost = document.createElement("span");
    ghost.className = "preview-revision-ghost";
    ghost.setAttribute("aria-hidden", "true");
    ghost.textContent = token.text;
    ghost.style.left = `${rect.left - rootRect.left + this.root.scrollLeft}px`;
    ghost.style.top = `${rect.top - rootRect.top + this.root.scrollTop}px`;
    ghost.style.width = `${rect.width}px`;
    ghost.style.height = `${rect.height}px`;
    while (this.revisionGhosts.length >= this.revisionGhostLimit) {
      this.revisionGhosts.shift()?.remove();
    }
    this.revisionGhosts.push(ghost);
    this.root.append(ghost);

    const animation = ghost.animate(
      [
        { opacity: 0.78, transform: "scale(1)" },
        { opacity: 0, transform: "scale(.82)", offset: 1 },
      ],
      { duration: 250, easing: "cubic-bezier(.35,0,.65,1)" },
    );
    const removeGhost = () => {
      ghost.remove();
      this.revisionGhosts = this.revisionGhosts.filter((candidate) => candidate !== ghost);
    };
    animation.finished.then(removeGhost).catch(removeGhost);
  }

  private animationsEnabled(): boolean {
    return this.hasRendered && this.supportsWebAnimations && !this.reducedMotion.matches;
  }
}
