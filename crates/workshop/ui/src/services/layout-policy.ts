// The layout policy contract: the product's default arrangement, which
// the layout core applies without naming a feature. The composition root
// registers the product's policy before the dock boots; the boot decision
// (parts/layout/layout-boot.ts) and the Secondary Side Bar toggle resolve
// it. No default is registered, so a bundle that applies a layout without
// binding a policy fails naming the token.

import type { ZoneName } from "@workshop/platform/panel-registry";
import { createServiceToken, type ServiceToken } from "@workshop/platform/service-registry";

/** The product's default layout and the panels the workbench never boots without. */
export interface LayoutPolicy {
  /**
   * Panel types re-opened after every layout apply, so a restored layout
   * that lost one gets it back. Each opens with empty params, so it is a
   * singleton and re-opening an open one only focuses it.
   */
  readonly anchors: readonly string[];
  /**
   * Zones that always have a group, even with no panel in it: after every
   * layout apply, a zone with no live group gets an empty one. A layout
   * saved before the zone had one gains it on restore.
   */
  readonly emptyZones?: readonly ZoneName[];
  /** Opens the default layout into a blank dock. */
  seed(): void;
}

/** The registry token for the product's layout policy. */
export const LAYOUT_POLICY: ServiceToken<LayoutPolicy> = createServiceToken<LayoutPolicy>("workshop.layoutPolicy");
