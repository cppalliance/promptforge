// The workspace-document feature's entry point: register() is the
// composition root's activation hook. Importers point at the source
// files directly; this module re-exports nothing.

import type { IDisposable } from "../../base/lifecycle";
import { registrations } from "./workspace-document.contribution";

/**
 * The workspace-document directory's activation. The actions register
 * eagerly from workspace-document.contribution.ts (imported by the
 * contribution surface); this hands their registrations to the caller
 * as one disposable, so the composition root's ownership tree tears the
 * feature down with the rest of the page. Called once from main.ts; the
 * returned disposable is held for the page lifetime.
 */
export function register(): IDisposable {
  return registrations;
}
