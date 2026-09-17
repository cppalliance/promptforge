// The Cloud tab's cascade derivation, pure over the cached sheet: no
// round trips. Providers group by tier in a fixed order for the
// dropdown; families scope to the selected kind and provider; the table
// lists canonical entries with their snapshot variants counted.
// Embedding and classifier entries never appear: the kind lozenges
// select only the five renderable kinds, so filtering by kind excludes
// them everywhere.

import type { CloudModelEntry, CloudProviderSlice, CloudSheet } from "./gateway-api";

/** The tier grouping order for the provider dropdown. */
const TIER_ORDER = ["prime", "subprime", "niche", "aggregator"] as const;

/** One provider option in the cascade's dropdown model. */
export interface CloudProviderOption {
  /** The provider's key in the sheet (and the endpoint id). */
  name: string;
  /** The UI-facing name. */
  displayName: string;
  /** True when the provider has no models of the selected kind. */
  disabled: boolean;
  /** The provider's sheet slice. */
  slice: CloudProviderSlice;
}

/** One non-empty tier group of provider options. */
export interface CloudTierGroup {
  /** The tier key: prime, subprime, niche, aggregator. */
  tier: string;
  /** The group's providers, alphabetical by display name. */
  providers: CloudProviderOption[];
}

/** One canonical table row: the entry plus its snapshot variants. */
export interface CloudCanonicalRow {
  /** The canonical entry (no `variant_of`). */
  entry: CloudModelEntry;
  /** The entry's variants, in sheet order. */
  variants: CloudModelEntry[];
}

/** The name cell's display rule output. */
export interface CloudDisplayName {
  /** The primary text: the display name. */
  primary: string;
  /** The id, shown beneath in monospace when it differs. */
  secondary: string | null;
}

/**
 * The provider dropdown model: one group per non-empty tier in the
 * order Prime, Subprime, Niche, Aggregator, alphabetical by display
 * name within each, with providers that serve nothing of the selected
 * kind flagged for greying.
 */
export function providersByTier(sheet: CloudSheet, kind: string): CloudTierGroup[] {
  const groups: CloudTierGroup[] = [];
  for (const tier of TIER_ORDER) {
    const providers = Object.entries(sheet.providers)
      .filter(([, slice]) => slice.tier === tier)
      .map(([name, slice]) => ({
        name,
        displayName: slice.display_name,
        disabled: !slice.models.some((entry) => entry.kind === kind),
        slice,
      }))
      .sort((left, right) => left.displayName.localeCompare(right.displayName));
    if (providers.length > 0) {
      groups.push({ tier, providers });
    }
  }
  return groups;
}

/** The distinct family values of one provider's models of `kind`, sorted. */
export function familiesFor(slice: CloudProviderSlice, kind: string): string[] {
  const families = new Set<string>();
  for (const entry of slice.models) {
    if (entry.kind === kind && entry.family !== "") {
      families.add(entry.family);
    }
  }
  return [...families].sort((left, right) => left.localeCompare(right));
}

/**
 * The table rows for one provider: canonical entries of `kind` (a
 * family selection narrows them), each carrying its snapshot variants
 * for the "+N snapshots" disclosure.
 */
export function canonicalRows(
  slice: CloudProviderSlice,
  kind: string,
  family: string | null,
): CloudCanonicalRow[] {
  const variantsOf = new Map<string, CloudModelEntry[]>();
  for (const entry of slice.models) {
    if (entry.variant_of !== null) {
      const list = variantsOf.get(entry.variant_of) ?? [];
      list.push(entry);
      variantsOf.set(entry.variant_of, list);
    }
  }
  return slice.models
    .filter(
      (entry) =>
        entry.variant_of === null &&
        entry.kind === kind &&
        (family === null || entry.family === family),
    )
    .map((entry) => ({ entry, variants: variantsOf.get(entry.id) ?? [] }));
}

/**
 * The name cell rule: the display name is primary; the id rides beneath
 * it in monospace when they differ, since the id is what `upstream`
 * receives. Providers reporting no display name show the id once.
 */
export function displayRule(entry: CloudModelEntry): CloudDisplayName {
  return {
    primary: entry.display_name,
    secondary: entry.display_name === entry.id ? null : entry.id,
  };
}
