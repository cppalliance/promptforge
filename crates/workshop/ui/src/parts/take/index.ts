// The take directory's barrel. The take registry is not a lazy feature -
// it has no panel, so this module holds no register(). Importers use
// take-registry.ts, whose exports are the registry's API; this barrel
// re-exports every module, reducer internals included.
export * from "./take-registry-events";
export * from "./take-registry-state";
export * from "./take-registry-types";
export * from "./take-registry";
