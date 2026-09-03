// esbuild bundles CSS imported from TypeScript into dist/app.css; tsc
// does not know the .css extension, so the side-effect imports in
// main.ts need this ambient declaration to typecheck.
declare module "*.css";
