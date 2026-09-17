// The Secrets tab's provider accessor: reads the loaded cloud sheet's
// provider slices and exposes them grouped by tier for the
// add-variable dropdown. It knows no provider by name and holds no
// table; every name, tier, and variable comes from the sheet. Greying
// and the selection fill derive from the slice's `env_vars` against the
// variable names already present in the env file.

import type { CloudProviderSlice, CloudSheet } from "./gateway-api";

/** The tier grouping order, matching the Cloud tab's dropdown. */
const TIER_ORDER = ["prime", "subprime", "niche", "aggregator"] as const;

/** One provider option in the Secrets add-variable dropdown. */
export interface SecretProviderOption {
  /** The provider's key in the sheet. */
  name: string;
  /** The UI-facing name. */
  displayName: string;
  /**
   * True when a selection could add nothing: the slice declares no
   * key-role variable, or every key-role variable is already present.
   */
  disabled: boolean;
  /** The provider's sheet slice. */
  slice: CloudProviderSlice;
}

/** One non-empty tier group of provider options. */
export interface SecretProviderTierGroup {
  /** The tier key: prime, subprime, niche, aggregator. */
  tier: string;
  /** The group's providers, alphabetical by display name. */
  providers: SecretProviderOption[];
}

/** What a dropdown selection stages in the add-variable row. */
export interface SecretProviderSelection {
  /**
   * The first key-role variable not already present, for the NAME
   * input; null when the provider has none to offer.
   */
  name: string | null;
  /**
   * One row per further `env_vars` entry not already present: config
   * roles prefill their default, key roles stay empty.
   */
  rows: Array<{ key: string; value: string }>;
}

/**
 * The dropdown model: one group per non-empty tier in the order Prime,
 * Subprime, Niche, Aggregator, alphabetical by display name within
 * each. Every slice appears regardless of status; `presentKeys` (the
 * variable names already in the env file) drives the greying.
 */
export function secretProvidersByTier(
  sheet: CloudSheet,
  presentKeys: ReadonlySet<string>,
): SecretProviderTierGroup[] {
  const groups: SecretProviderTierGroup[] = [];
  for (const tier of TIER_ORDER) {
    const providers = Object.entries(sheet.providers)
      .filter(([, slice]) => slice.tier === tier)
      .map(([name, slice]) => {
        const keyVars = slice.env_vars.filter((entry) => entry.role === "key");
        return {
          name,
          displayName: slice.display_name,
          disabled:
            keyVars.length === 0 || keyVars.every((entry) => presentKeys.has(entry.name)),
          slice,
        };
      })
      .sort((left, right) => left.displayName.localeCompare(right.displayName));
    if (providers.length > 0) {
      groups.push({ tier, providers });
    }
  }
  return groups;
}

/**
 * The fill for one selection: the slice's first key-role variable not
 * already present goes to NAME, and each further `env_vars` entry not
 * already present appends a row - config roles prefilled with their
 * default, key roles empty. Already-present entries are skipped so a
 * selection never duplicates an existing variable.
 */
export function secretProviderSelection(
  slice: CloudProviderSlice,
  presentKeys: ReadonlySet<string>,
): SecretProviderSelection {
  const name =
    slice.env_vars.find((entry) => entry.role === "key" && !presentKeys.has(entry.name))?.name ??
    null;
  if (name === null) {
    return { name, rows: [] };
  }
  const rows = slice.env_vars
    .filter((entry) => entry.name !== name && !presentKeys.has(entry.name))
    .map((entry) => ({
      key: entry.name,
      value: entry.role === "config" ? (entry.default ?? "") : "",
    }));
  return { name, rows };
}
