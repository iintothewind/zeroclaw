/**
 * ESM loader hook that lets jiti-based tests (npm run test:contexts and the
 * pretest hook that drives src/contexts/sessionLifecycle.test.ts) import
 * CSS module files. Node's native ESM loader throws on `import './X.css'`;
 * jiti falls through to `import()` for unknown extensions, which is what
 * triggers this hook.
 *
 * Behaviour: any specifier ending in `.css` is resolved to a stub module
 * that exports a Proxy. Class-name lookups (`css.attach`, `css.row`, ...)
 * return the key as a string, so the React tree can apply `className` values
 * without crashing. Tests that introspect the rendered tree do not assert
 * on the exact class strings, so this is enough for the jiti path; the
 * production build still goes through Vite's real CSS-module pipeline.
 *
 * Activate with `node --import ./scripts/css-stub-loader.mjs ...`.
 */

const STUB_MODULE_URL =
  "data:text/javascript;base64," +
  Buffer.from(
    'const stub = new Proxy({}, { get(_, k) { return typeof k === "symbol" ? undefined : String(k); } }); export default stub;',
  ).toString("base64");

export function resolve(specifier, context, nextResolve) {
  if (process.env.CSS_STUB_DEBUG) {
    console.error(`[css-stub] resolve: ${specifier} (parent=${context.parentURL})`);
  }
  if (specifier.endsWith(".css")) {
    return {
      url: STUB_MODULE_URL,
      shortCircuit: true,
      format: "module",
    };
  }
  return nextResolve(specifier, context);
}