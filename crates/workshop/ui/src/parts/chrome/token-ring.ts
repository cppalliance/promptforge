// The token ring: a 15px SVG gauge showing how much of the model's
// context window the session has used. Two concentric circles - a track
// and a progress arc driven by stroke-dashoffset - with stroke 2, radius
// 5.5, and round line caps, sitting in a 20px box. The percentage comes
// from a constructor-injected provider; the default stub reports no usage
// data (null) until a real context-usage service replaces it, and the ring
// stays hidden while it has none. setPercentage is the push path that
// service (and tests) use.

import "./token-ring.css";

import { Disposable } from "@workshop/platform/lifecycle";

/**
 * Supplies the context-usage percentage (0-100) the ring displays, or
 * null while there is no usage data, which hides the ring.
 */
export type TokenRingPercentageProvider = () => number | null;

/** The stub source: no context-usage service exists yet, so no data. */
const stubProvider: TokenRingPercentageProvider = () => null;

const SVG_NAMESPACE = "http://www.w3.org/2000/svg";
const VIEW_BOX_SIZE = 15;
const STROKE_WIDTH = 2;
const CENTER = VIEW_BOX_SIZE / 2;
const RADIUS = 5.5;
const CIRCUMFERENCE = 2 * Math.PI * RADIUS;

/** Clamps an incoming percentage into 0-100; a non-finite value reads as empty. */
function clampPercentage(percentage: number): number {
  if (!Number.isFinite(percentage)) {
    return 0;
  }
  return Math.min(100, Math.max(0, percentage));
}

/**
 * The ring gauge. The role is progressbar rather than img because the
 * ring reports a live scalar value: aria-valuenow keeps the percentage
 * available to assistive tech where an image role would hide it. With no
 * usage data the ring is hidden and carries no value. Disposable for
 * uniform toolbar lifetime; it owns no listeners.
 */
export class TokenRing extends Disposable {
  /** The SVG element; append it where the ring belongs. */
  readonly element: SVGSVGElement;

  private readonly provider: TokenRingPercentageProvider;
  private readonly progressCircle: SVGCircleElement;
  private current: number | null;

  constructor(provider: TokenRingPercentageProvider = stubProvider) {
    super();
    this.provider = provider;

    this.element = document.createElementNS(SVG_NAMESPACE, "svg");
    this.element.setAttribute("class", "ws-token-ring");
    this.element.setAttribute("viewBox", `0 0 ${VIEW_BOX_SIZE} ${VIEW_BOX_SIZE}`);
    this.element.setAttribute("role", "progressbar");
    this.element.setAttribute("aria-label", "Context usage");
    this.element.setAttribute("aria-valuemin", "0");
    this.element.setAttribute("aria-valuemax", "100");

    const background = document.createElementNS(SVG_NAMESPACE, "circle");
    background.setAttribute("class", "ws-token-ring-background");
    background.setAttribute("cx", String(CENTER));
    background.setAttribute("cy", String(CENTER));
    background.setAttribute("r", String(RADIUS));
    background.setAttribute("stroke-width", String(STROKE_WIDTH));

    this.progressCircle = document.createElementNS(SVG_NAMESPACE, "circle");
    this.progressCircle.setAttribute("class", "ws-token-ring-progress");
    this.progressCircle.setAttribute("cx", String(CENTER));
    this.progressCircle.setAttribute("cy", String(CENTER));
    this.progressCircle.setAttribute("r", String(RADIUS));
    this.progressCircle.setAttribute("stroke-width", String(STROKE_WIDTH));
    this.progressCircle.setAttribute("stroke-linecap", "round");
    this.progressCircle.setAttribute("stroke-dasharray", String(CIRCUMFERENCE));

    this.element.append(background, this.progressCircle);

    const initial = this.provider();
    this.current = initial === null ? null : clampPercentage(initial);
    this.renderPercentage();
  }

  /** The displayed percentage, clamped to 0-100, or null while there is no usage data. */
  get percentage(): number | null {
    return this.current;
  }

  /** Pushes a new percentage (null for no data) and re-renders the arc. */
  setPercentage(percentage: number | null): void {
    this.current = percentage === null ? null : clampPercentage(percentage);
    this.renderPercentage();
  }

  private renderPercentage(): void {
    if (this.current === null) {
      // No usage data: the ring is hidden and reports no value.
      this.element.setAttribute("hidden", "");
      this.element.removeAttribute("aria-valuenow");
      this.progressCircle.setAttribute("stroke-dashoffset", String(CIRCUMFERENCE));
      return;
    }
    this.element.removeAttribute("hidden");
    const offset = CIRCUMFERENCE * (1 - this.current / 100);
    this.progressCircle.setAttribute("stroke-dashoffset", String(offset));
    this.element.setAttribute("aria-valuenow", String(this.current));
  }
}
