/**
 * Tiny preload that registers the CSS-module stub loader with Node's ESM
 * loader-hook system. `--import` alone only imports a module; the module
 * must opt in to the loader-hook chain via `module.register()`.
 *
 * Spawn jiti-run.mjs with `--import` pointing at this file and Node will
 * route every subsequent `import './X.css'` through `css-stub-loader.mjs`.
 */

import { register } from "node:module";

register(new URL("./css-stub-loader.mjs", import.meta.url).href);