// node templates/canvy/render/client/surface.test.js
//
// The routing is the part of the view that was visibly wrong — lines ran under
// the cells they connected — so it is the part that gets a test rather than a
// look. Everything here is a property, not a pixel: no rendering, no browser.

const G = require("./surface.js");
const W = 150, H = 38;

let fails = 0;
function ok(name, cond, detail) {
  if (cond) { console.log(`  ok   ${name}`); return; }
  fails++;
  console.log(`  FAIL ${name}${detail ? " — " + detail : ""}`);
}

/// Parse a path back into the points it visits, so the test reasons about the
/// geometry rather than about the string.
function points(d) {
  const out = [];
  for (const m of d.matchAll(/[MLQ]\s*(-?[\d.]+),(-?[\d.]+)(?:\s+(-?[\d.]+),(-?[\d.]+))?/g)) {
    if (m[3] !== undefined) out.push({x: +m[3], y: +m[4]});   // Q: end point
    else out.push({x: +m[1], y: +m[2]});
  }
  return out;
}

console.log("edge routing");

// --- 1. the complaint that started this: a line must not run under its own boxes
{
  const cases = [
    ["target right",      {x: 0, y: 0},    {x: 400, y: 0}],
    ["target left",       {x: 400, y: 0},  {x: 0, y: 0}],
    ["target below",      {x: 0, y: 0},    {x: 0, y: 300}],
    ["target above",      {x: 0, y: 300},  {x: 0, y: 0}],
    ["diagonal",          {x: 0, y: 0},    {x: 320, y: 240}],
    ["barely offset",     {x: 0, y: 0},    {x: 300, y: 4}],
    ["overlapping rows",  {x: 0, y: 0},    {x: 180, y: 20}],
  ];
  for (const [name, a, b] of cases) {
    const {d} = G.route(a, b, W, H, 0);
    const pts = points(d);
    let inside = false;
    for (let i = 1; i < pts.length; i++) {
      if (G.segmentHitsBox(pts[i - 1], pts[i], a, W, H) ||
          G.segmentHitsBox(pts[i - 1], pts[i], b, W, H)) inside = true;
    }
    ok(`clear of both boxes (${name})`, !inside, d);
  }
}

// --- 2. it starts and ends ON a boundary, so the arrowhead is visible
{
  const a = {x: 0, y: 0}, b = {x: 400, y: 120};
  const {start, end} = G.route(a, b, W, H, 0);
  const onEdge = (p, box) => {
    const dx = Math.min(Math.abs(p.x - box.x), Math.abs(p.x - (box.x + W)));
    const dy = Math.min(Math.abs(p.y - box.y), Math.abs(p.y - (box.y + H)));
    return dx < 0.6 || dy < 0.6;
  };
  ok("start sits on the source boundary", onEdge(start, a), JSON.stringify(start));
  ok("end sits on the target boundary", onEdge(end, b), JSON.stringify(end));
}

// --- 3. parallel edges between the same pair do not collapse into one line
{
  const a = {x: 0, y: 0}, b = {x: 400, y: 0};
  const r0 = G.route(a, b, W, H, 0), r1 = G.route(a, b, W, H, 1), rm = G.route(a, b, W, H, -1);
  ok("lane 1 differs from lane 0", r0.d !== r1.d);
  ok("lane -1 differs from lane 0", r0.d !== rm.d);
  ok("lanes stay on the same side", Math.abs(r0.start.x - r1.start.x) < 0.6,
     `${r0.start.x} vs ${r1.start.x}`);
  ok("lane offsets are bounded to the box", Math.abs(r1.start.y - r0.start.y) <= H / 2);
}

// --- 4. every turn is a right angle (orthogonal, not diagonal)
{
  const {d} = G.route({x: 0, y: 0}, {x: 340, y: 260}, W, H, 0);
  const pts = points(d);
  let diagonal = 0;
  for (let i = 1; i < pts.length; i++) {
    const dx = Math.abs(pts[i].x - pts[i - 1].x), dy = Math.abs(pts[i].y - pts[i - 1].y);
    // A rounded corner is one Q whose ends differ on both axes by <= the radius.
    if (dx > 7 && dy > 7) diagonal++;
  }
  ok("no diagonal segments", diagonal === 0, `${diagonal} diagonal`);
}

// --- 5. the side chosen actually faces the target
{
  ok("wide box prefers top/bottom when target is below",
     G.side({x: 0, y: 0}, {x: 10, y: 300}, W, H).y === 1);
  ok("wide box prefers left/right when target is beside",
     Math.abs(G.side({x: 0, y: 0}, {x: 400, y: 10}, W, H).x) === 1);
  ok("exits towards a target above",
     G.side({x: 0, y: 300}, {x: 0, y: 0}, W, H).y === -1);
}

// --- 6. a degenerate case must still produce a drawable path
{
  const {d} = G.route({x: 100, y: 100}, {x: 100, y: 100}, W, H, 0);
  ok("self-overlap still yields a path", /^M[-\d.]+,[-\d.]+/.test(d) && d.length > 10, d);
  ok("no NaN in output", !/NaN/.test(d), d);
}

// --- 7. the ONE expression the hook evaluates
//
// Every case above calls `route(...)` and reads `.d`. The hook did not: it said
// `rounded(route(...))`, and `rounded` takes an array of points, so what reached
// the browser was `MNaN,NaN` — an edge element with an unusable path, i.e. no
// visible lines at all, on every join since the surface existed. Nineteen green
// property tests said nothing about it, because none of them evaluated the line
// the client actually runs. `edgePath` is now that line, and this is its test.
{
  const d = G.edgePath({x: 0, y: 0}, {x: 400, y: 260}, W, H, 0);
  ok("edgePath yields a drawable path", /^M[-\d.]+,[-\d.]+/.test(d), d);
  ok("edgePath has no NaN", !/NaN/.test(d), d);
  ok("edgePath is what route promises", d === G.route({x: 0, y: 0}, {x: 400, y: 260}, W, H, 0).d);
  // The shape of the old defect, pinned so nobody reintroduces it: handing the
  // route OBJECT to the point-list function does not merely draw badly, it
  // THROWS — which is why not one edge in the picture had a path. An exception
  // inside the loop takes every remaining edge with it.
  let threw = false;
  try {
    G.rounded(G.route({x: 0, y: 0}, {x: 400, y: 260}, W, H, 0));
  } catch (e) {
    threw = true;
  }
  ok("rounded over a route object throws — the old defect", threw);
}

// --- 8. THE HOOK ITSELF, mounted against a fake DOM
//
// Everything above tests geometry. Nothing tested the hook, and the hook is where
// both browser-visible defects lived: it was never mounted (the markup offered no
// `phx-hook`), and when it did run its edge call threw. A canvas with no lines
// that cannot be dragged passed every test this file had.
//
// There is no browser and no DOM library here — the tech stack of this repo is a
// closed list and a canvas is not a reason to open it. So the DOM is the six
// methods the hook actually calls, hand-built, over the same attribute names the
// server emits. That is enough to answer the two questions that matter: does every
// edge get a usable path, and does letting go of a box tell the server where it
// landed.
console.log("\nthe hook");
{
  global.document = {};                       // the hook's own guard needs this
  // The drag coalesces its DOM writes into one animation frame. Run it straight
  // through: the test wants the arithmetic, not the scheduling.
  global.requestAnimationFrame = (f) => { f(); return 1; };
  delete require.cache[require.resolve("./surface.js")];
  require("./surface.js");
  const Canvy = (global.SurfaceHooks || globalThis.SurfaceHooks).Canvy;
  ok("surface.js registers the Canvy hook", !!Canvy);

  function elem(attrs, tag) {
    return {
      tag: tag || "g",
      attrs: Object.assign({}, attrs),
      getAttribute(k) { return k in this.attrs ? String(this.attrs[k]) : null; },
      setAttribute(k, v) { this.attrs[k] = v; },
      removeAttribute(k) { delete this.attrs[k]; },
      closest(sel) { return sel === "[data-node]" && "data-node" in this.attrs ? this : null; },
    };
  }

  // Two cells, one edge between them, one viewport — the smallest real picture.
  const a = elem({"data-node": "a/one", transform: "translate(24,30)"});
  const b = elem({"data-node": "a/two", transform: "translate(400,300)"});
  const e = elem({"data-from": "a/one", "data-to": "a/two", "data-lane": "0"}, "path");
  const vp = elem({"data-cx": "0", "data-cy": "0", "data-cz": "1000"});
  const el = {
    classList: {add() {}, remove() {}},
    listeners: {},
    addEventListener(n, f) { this.listeners[n] = f; },
    removeEventListener(n) { delete this.listeners[n]; },
    querySelector(sel) { return sel === "g.viewport" ? vp : null; },
    querySelectorAll(sel) {
      if (sel === "[data-node]") return [a, b];
      if (sel === "path.edge") return [e];
      if (sel.startsWith("[data-from=")) {
        const id = sel.match(/"([^"]+)"/)[1];
        return [e].filter(p => p.getAttribute("data-from") === id ||
                               p.getAttribute("data-to") === id);
      }
      return [];
    },
  };

  const sent = [];
  const hook = Object.create(Canvy);
  hook.el = el;
  hook.pushEvent = (name, payload) => sent.push({name, payload});
  hook.mounted();

  const d = e.getAttribute("d");
  ok("mounting fills in the edge path", !!d && /^M[-\d.]+,[-\d.]+/.test(d), String(d));
  ok("and it carries no NaN", !!d && !/NaN/.test(d), String(d));
  ok("mounting applies the camera", /translate\(0,0\) scale\(1/.test(vp.getAttribute("transform") || ""));

  // A drag: press on a box, move, let go. One event, carrying where it landed.
  el.listeners.pointerdown({target: a, clientX: 100, clientY: 100, preventDefault() {}});
  el.listeners.pointermove({target: a, clientX: 160, clientY: 140});
  el.listeners.pointerup({target: a, clientX: 160, clientY: 140});
  ok("letting go of a box sends exactly one event", sent.length === 1, JSON.stringify(sent));
  ok("and it is node:moved with the drop position",
     sent.length === 1 && sent[0].name === "node:moved" &&
     sent[0].payload.id === "a/one" &&
     sent[0].payload.x === 84 && sent[0].payload.y === 70,
     JSON.stringify(sent[0]));

  // Pressing the empty canvas pans instead, and sends nothing.
  const before = vp.getAttribute("transform");
  el.listeners.pointerdown({target: elem({}), clientX: 10, clientY: 10, preventDefault() {}});
  el.listeners.pointermove({clientX: 60, clientY: 30});
  el.listeners.pointerup({clientX: 60, clientY: 30});
  ok("dragging the empty canvas pans the view", vp.getAttribute("transform") !== before,
     vp.getAttribute("transform"));
  ok("and panning tells the server nothing", sent.length === 1);
}

console.log(fails ? `\n${fails} failing` : "\nall green");
process.exit(fails ? 1 : 0);
