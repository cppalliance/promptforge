// Ambient declaration for the colocated-CSS pattern: components import
// their stylesheet for the side effect, the consuming UI's esbuild build
// extracts the CSS into its app.css, and tsc needs the module shape
// declared to accept the import. Side-effect only - no exports.
declare module "*.css";
