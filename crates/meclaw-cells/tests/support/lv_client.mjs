// The vendored LiveView client's own `Rendered`, driven from a test (GH #1001).
//
// Usage: node lv_client.mjs <phoenix_live_view.min.js> <steps.json>
//
// steps.json: {"join": <rendered of the join reply>, "diffs": [<push payload>, ...]}
//
// The client is loaded unchanged from disk; only the in-memory copy is given
// one extra export, the `Rendered` class the bundle keeps private, so the test
// reads exactly what a browser would build from the same frames. Each step is
// merged and rendered the way `View.update` does it (`mergeDiff`, then
// `toString` with change tracking), and the tracked output of every step is
// printed, so a test can see which parts the client skipped. `full` is the
// end state rendered without tracking: the whole markup, with the client's
// `data-phx-id` attributes on every root part. `fulls` is that untracked
// rendering after every step, so a test can follow one part's `data-phx-id`
// across a frame: the browser's patch keys its nodes by that id (GH #1009).
//
// Prints one JSON object {"steps": [html, ...], "fulls": [html, ...],
// "full": html} on stdout.
// Exit 3 with SKIP on stderr when the bundle does not have the expected shape.

import fs from "node:fs";
import vm from "node:vm";

const [clientPath, stepsPath] = process.argv.slice(2);
let src = fs.readFileSync(clientPath, "utf8");

const cls = src.match(/var (\w+)=class\{static extract\(e\)/);
const ret = src.match(/return (\w+)\((\w+)\);\}\)\(\);\s*$/);
if (!cls || !ret) {
  console.error("SKIP: the client bundle does not have the expected shape");
  process.exit(3);
}
src =
  src.slice(0, ret.index) +
  `return Object.assign({}, ${ret[1]}(${ret[2]}), {__Rendered: ${cls[1]}});})();`;

const window = { location: { href: "http://127.0.0.1/" } };
const context = vm.createContext({ window, console, structuredClone, URL, setTimeout, clearTimeout });
context.globalThis = context;
vm.runInContext(src + "\n;globalThis.LiveView = LiveView;", context);
const Rendered = context.LiveView.__Rendered;

const steps = JSON.parse(fs.readFileSync(stepsPath, "utf8"));
const out = { steps: [], fulls: [] };
const r = new Rendered("lab", steps.join);
const untracked = () => {
  const rendered = r.get();
  return r.recursiveToString(rendered, rendered.c, null, false, {}, null).buffer;
};
out.steps.push(r.toString().buffer);
out.fulls.push(untracked());
for (const diff of steps.diffs) {
  r.mergeDiff(diff);
  out.steps.push(r.toString().buffer);
  out.fulls.push(untracked());
}
out.full = untracked();
process.stdout.write(JSON.stringify(out));
