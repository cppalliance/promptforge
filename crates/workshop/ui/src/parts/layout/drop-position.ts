// The dock's drop-position resolver: where a dragged tab docks over a
// group or the whole layout, the way Cursor's workbench cuts a drop target.
// The target is cut in thirds - left, right, top, or bottom when the pointer
// is in that third alone, the center when it is in the middle ninth. A corner
// cell, where two thirds claim the pointer, goes to the nearer edge, measured
// against the target's own size so a wide target is not biased toward its
// long sides.
//
// A pointer within 10% of an edge docks against that edge, but that needs no
// rule of its own: such a pointer is inside that edge's third and nearer it
// than any other edge, so the thirds already answer with that edge. A band
// branch ahead of the thirds could never change an answer. What a band could
// add is Dockview's `edge` marker (a whole-layout dock), which this resolver
// leaves unused because Dockview draws no overlay for an edge cell and leaves
// the commit to the consumer.
//
// A target accepts only some positions (a tab strip takes just the center),
// so a position it refuses falls back to the center, and a target that takes
// nothing answers null: no drop. main.ts passes the resolver to Dockview's
// `dropPositionResolver` option, which reads it live for every group and for
// the whole-layout edges.

import type { Position, PositionResolver, PositionResolverArgs, PositionResolverResult } from "dockview";

/** The point where the left third ends and the middle third begins. */
const THIRD = 1 / 3;

/** What the resolver reads of Dockview's arguments; the pointer's own event is not needed. */
type DropPoint = Pick<PositionResolverArgs, "x" | "y" | "width" | "height" | "zones">;

/** The four edges, with the pointer's distance to each as a fraction of that axis. */
interface EdgeDistances {
  readonly left: number;
  readonly right: number;
  readonly top: number;
  readonly bottom: number;
}

/** The nearest of two edges on one axis, with its distance. */
function nearer(first: [Position, number], second: [Position, number]): [Position, number] {
  return first[1] <= second[1] ? first : second;
}

/** The edge that claims the pointer: the one whose third it is in, else the center. */
function edgeFor(distance: EdgeDistances): Position {
  const horizontal = nearer(["left", distance.left], ["right", distance.right]);
  const vertical = nearer(["top", distance.top], ["bottom", distance.bottom]);

  // An edge's third is the part of the target within a third of it. Two thirds
  // overlap at a corner, where the nearer edge wins.
  const inHorizontalThird = horizontal[1] < THIRD;
  const inVerticalThird = vertical[1] < THIRD;
  if (inHorizontalThird && inVerticalThird) {
    return nearer(horizontal, vertical)[0];
  }
  if (inHorizontalThird) {
    return horizontal[0];
  }
  if (inVerticalThird) {
    return vertical[0];
  }
  return "center";
}

/**
 * Resolves a pointer location within a drop target to the position it docks
 * at, or null when the target accepts no drop.
 */
export function resolveDropPosition(point: DropPoint): PositionResolverResult | null {
  const { x, y, width, height, zones } = point;
  const accept = (position: Position): PositionResolverResult | null => {
    if (zones.has(position)) {
      return { position };
    }
    return zones.has("center") ? { position: "center" } : null;
  };
  if (!(width > 0) || !(height > 0)) {
    return accept("center");
  }
  const fractionX = Math.min(Math.max(x / width, 0), 1);
  const fractionY = Math.min(Math.max(y / height, 0), 1);
  return accept(
    edgeFor({
      left: fractionX,
      right: 1 - fractionX,
      top: fractionY,
      bottom: 1 - fractionY,
    }),
  );
}

/** The resolver main.ts hands Dockview. */
export const dropPositionResolver: PositionResolver = {
  resolve: (args) => resolveDropPosition(args),
};
