// node templates/canvy/layout/canvy.test.js
//
// The client's own suite, run by `crates/meclaw-cells/tests/canvy2_client_geometry.rs`
// so that `cargo test` executes it. A test file that only exists is a comment:
// before 2026-08-17 the geometry had 19 green property tests, the hook had none,
// and a browser found both defects the same afternoon. The 0.12.1 lesson stands
// for 2.0.0 unchanged — the client path is never proven over the websocket alone.
//
// Everything here is a property, not a pixel: no rendering, no browser, and no
// DOM library (the repo's tech stack is a closed list and a canvas is not a
// reason to open it). The DOM is the handful of methods the hook actually calls,
// hand-built over the attribute names the layout cell writes.

const G = require("./canvy.js");
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

// --- 7b. routing through the gaps the layout leaves
//
// The turn used to happen at the midpoint between the two boxes, which is wherever
// they happen to average out — on the live colony that put 47% of the lines
// straight through cells they had nothing to do with, and a picture whose lines
// run under unrelated boxes cannot be followed with the eye. The layout leaves
// gaps between its columns on purpose; the router now turns in one of them.
{
  // Source left, target right, and an unrelated box parked exactly on the
  // midpoint — the case that produced the crossing.
  const a = {x: 0, y: 0}, b = {x: 800, y: 200};
  const blocker = {x: 380, y: 0};
  const lanes = G.freeLanes([a, b, blocker], W, H);
  const naive = points(G.edgePath(a, b, W, H, 0));
  const routed = points(G.edgePath(a, b, W, H, 0, lanes));
  const hits = (pts) => {
    for (let i = 1; i < pts.length; i++) {
      if (G.segmentHitsBox(pts[i - 1], pts[i], blocker, W, H)) return true;
    }
    return false;
  };
  ok("the midpoint turn runs through the box in the way", hits(naive));
  ok("routing through a free corridor does not", !hits(routed));

  // A corridor is only worth taking if it lies on the way. One far off to the
  // side must be ignored rather than turned into a detour.
  const far = G.corridor([-5000, 9000], 0, 800);
  ok("a corridor outside the span is ignored", far === 400, String(far));
  ok("with no corridors at all it is still the midpoint",
     G.corridor([], 0, 800) === 400 && G.corridor(null, 0, 800) === 400);

  // A box sitting ON the line the anchors leave along cannot be routed around by
  // any choice of turning point — the way past it is to step aside first. That
  // path costs one more bend, so it is offered only when the plain one fails.
  {
    const src = {x: 0, y: 0}, dst = {x: 900, y: 0};
    const wall = {x: 300, y: 0};                       // same row, dead ahead
    const ctx = G.freeLanes([src, dst, wall], W, H);
    const p = points(G.edgePath(src, dst, W, H, 0, ctx));
    let hit = false;
    for (let i = 1; i < p.length; i++) {
      if (G.segmentHitsBox(p[i - 1], p[i], wall, W, H)) hit = true;
    }
    ok("a box straight ahead is stepped around, not driven through", !hit,
       JSON.stringify(p));
    // ...and the plain route keeps its right of way when nothing is in the way:
    // a picture where every line zigzags reads no better than one where they run
    // under the boxes.
    const clear = G.freeLanes([src, dst], W, H);
    const plain = points(G.edgePath(src, dst, W, H, 0, clear));
    let turns = 0;
    for (let i = 2; i < plain.length; i++) {
      const before = Math.abs(plain[i-1].x - plain[i-2].x) > Math.abs(plain[i-1].y - plain[i-2].y);
      const after = Math.abs(plain[i].x - plain[i-1].x) > Math.abs(plain[i].y - plain[i-1].y);
      if (before !== after) turns++;
    }
    ok("an unobstructed run stays straight", turns === 0, `${turns} turns`);
  }

  // And the free bands are the gaps BETWEEN the boxes, never inside one.
  const bands = G.freeLanes([{x: 0, y: 0}, {x: 400, y: 0}], W, H).x;
  ok("two boxes leave one gap between them, plus the open ground on either side",
     bands.length === 3, JSON.stringify(bands));
  ok("the inner lane sits between them",
     bands.some(v => v > 150 && v < 400), JSON.stringify(bands));
  ok("and the outer lanes clear everything",
     bands.some(v => v < 0) && bands.some(v => v > 550), JSON.stringify(bands));
}

// --- 8. THE HOOK ITSELF, mounted against a fake DOM
//
// Everything above tests geometry. The hook is where both browser-visible
// defects of 1.x lived: it was never mounted (the markup offered no `phx-hook`),
// and when it did run its edge call threw. A canvas with no lines that cannot be
// dragged passed every geometry test there was.
//
// What is different in 2.0.0 is what leaves the browser. There is no
// `node:moved`, no `hive:moved`, no `camera:moved` and no `canvas:sweep`: a drag
// is an `object:set` on the two props the node component declared `editable`, a
// hive drag is that gesture repeated over its members, and the camera is local
// state that is never sent at all.
console.log("\nthe hook");
{
  // A document that can carry a listener, because Escape is bound there: a
  // selection has to be releasable without hunting for empty canvas to click.
  const docListeners = {};
  global.document = {
    addEventListener(n, f) { docListeners[n] = f; },
    removeEventListener(n) { delete docListeners[n]; },
  };
  // Nothing in this client debounces any more, but a clock the test can advance
  // is what proves it: a camera write that was merely late would still show up
  // when the queue is drained.
  const timers = [];
  global.setTimeout = (f) => { timers.push(f); return timers.length; };
  global.clearTimeout = () => {};
  const tick = () => { const q = timers.splice(0); q.forEach(f => f()); };
  // The drag coalesces its DOM writes into one animation frame. The shim QUEUES
  // like the real thing rather than running inline: a synchronous shim clears the
  // hook's `frame` guard before the hook assigns it, so the guard stays set and
  // every later move is dropped — which is exactly how the hive drag below first
  // "failed" while the code was right.
  const raf = [];
  global.requestAnimationFrame = (f) => { raf.push(f); return raf.length; };
  const flush = () => { const q = raf.splice(0); q.forEach(f => f()); };
  delete require.cache[require.resolve("./canvy.js")];
  require("./canvy.js");
  const Canvy = (global.SurfaceHooks || globalThis.SurfaceHooks).Canvy;
  ok("canvy.js registers the Canvy hook", !!Canvy);

  function elem(attrs, tag) {
    return {
      tag: tag || "g",
      attrs: Object.assign({}, attrs),
      getAttribute(k) { return k in this.attrs ? String(this.attrs[k]) : null; },
      setAttribute(k, v) { this.attrs[k] = v; },
      removeAttribute(k) { delete this.attrs[k]; },
      // Attribute selectors only, and matched against this element alone: the
      // stub has no parents. That is enough for every `closest` the hook makes,
      // and the `#detail` lookup keeps answering null exactly as it did.
      closest(sel) {
        const m = /^\[([\w-]+)/.exec(sel);
        return m && m[1] in this.attrs ? this : null;
      },
    };
  }

  // Two cells, one edge between them, one viewport — the smallest real picture.
  // `data-oid` is the OBJECT the cell is stored as; `data-node` is its colony
  // path. The two are different on purpose: the path is what an edge names, the
  // object id is what a patch names, and a picture that conflated them could not
  // hold a hive frame and a cell of the same name.
  const a = elem({"data-node": "a/one", "data-oid": "n/a/one",
                  transform: "translate(24,30)"});
  const b = elem({"data-node": "a/two", "data-oid": "n/a/two",
                  transform: "translate(400,300)"});
  const e = elem({"data-from": "a/one", "data-to": "a/two", "data-lane": "0"}, "path");
  const vp = elem({});
  // The frames present in the picture. Empty until the hive is introduced below,
  // which is also a test: a picture with no hives must not blow up on a drag.
  let hives = [];
  const el = {
    classList: {add() {}, remove() {}},
    // The geometry the layout cell wrote — the client reads it rather than
    // keeping a second copy of the numbers.
    attrs: {"data-nw": "150", "data-nh": "38", "data-pad-side": "24",
            "data-pad-top": "30", "data-pad-bot": "24", "data-nest": "18"},
    getAttribute(k) { return k in this.attrs ? this.attrs[k] : null; },
    listeners: {},
    addEventListener(n, f) { this.listeners[n] = f; },
    removeEventListener(n) { delete this.listeners[n]; },
    querySelector(sel) { return sel === "g.viewport" ? vp : null; },
    querySelectorAll(sel) {
      if (sel === "[data-node]") return [a, b];
      if (sel === "path.edge") return [e];
      if (sel === "[data-hive]") return hives;
      if (sel.startsWith("[data-from=")) {
        const id = sel.match(/"([^"]+)"/)[1];
        return [e].filter(p => p.getAttribute("data-from") === id ||
                               p.getAttribute("data-to") === id);
      }
      return [];
    },
  };

  function boxAttr(g) {
    const m = /translate\((-?[\d.]+),(-?[\d.]+)\)/.exec(g.getAttribute("transform"));
    return {x: +m[1], y: +m[2]};
  }

  const sent = [];
  const hook = Object.create(Canvy);
  hook.el = el;
  hook.pushEvent = (name, payload) => sent.push({name, payload});
  hook.mounted();

  const d = e.getAttribute("d");
  ok("mounting fills in the edge path", !!d && /^M[-\d.]+,[-\d.]+/.test(d), String(d));
  ok("and it carries no NaN", !!d && !/NaN/.test(d), String(d));
  ok("mounting applies the identity camera",
     /translate\(0,0\) scale\(1/.test(vp.getAttribute("transform") || ""));

  // A drag: press on a box, move, let go. Two events, one per editable prop,
  // naming the OBJECT and the value it now has.
  el.listeners.pointerdown({target: a, clientX: 100, clientY: 100, preventDefault() {}});
  el.listeners.pointermove({target: a, clientX: 160, clientY: 140});
  flush();
  el.listeners.pointerup({target: a, clientX: 160, clientY: 140});
  ok("letting go of a box sends two events — one per editable prop",
     sent.length === 2, JSON.stringify(sent));
  ok("both are object:set on the dragged OBJECT",
     sent.every(s => s.name === "object:set" && s.payload.id === "n/a/one"),
     JSON.stringify(sent));
  ok("and they carry the drop position",
     sent[0].payload.prop === "x" && sent[0].payload.value === 84 &&
     sent[1].payload.prop === "y" && sent[1].payload.value === 70,
     JSON.stringify(sent));
  sent.length = 0;

  // Pressing the empty canvas pans instead, and sends nothing.
  const before = vp.getAttribute("transform");
  el.listeners.pointerdown({target: elem({}), clientX: 10, clientY: 10, preventDefault() {}});
  el.listeners.pointermove({clientX: 60, clientY: 30});
  el.listeners.pointerup({clientX: 60, clientY: 30});
  ok("dragging the empty canvas pans the view", vp.getAttribute("transform") !== before,
     vp.getAttribute("transform"));
  tick();
  ok("and panning tells the cell nothing — the camera is local state",
     sent.length === 0, JSON.stringify(sent));

  // Dragging a HIVE: the frame, its label and every member move together, and one
  // patch pair goes out per member. There is no group row to write — the members'
  // own positions ARE the record, which is what removes the whole shift/anchor
  // apparatus 1.x needed (GH #170).
  const rect = elem({x: "0", y: "0", width: "300", height: "100"}, "rect");
  const label = elem({x: "8", y: "18"}, "text");
  const hiveG = elem({"data-hive": "a"});
  hiveG.classList = {add() {}, remove() {}};
  hiveG.querySelector = (sel) => (sel === "rect" ? rect : sel === "text" ? label : null);
  hiveG.closest = function (sel) { return sel === "[data-hive]" ? this : null; };
  hives = [hiveG];

  // A cell dragged out of its hive GROWS the hive, while the cursor is still
  // down. The frame is derived from the cells, so it can be derived again every
  // frame — and a frame that only catches up on release is a frame that lies for
  // as long as you are looking at it.
  el.listeners.pointerdown({target: a, clientX: 100, clientY: 100, preventDefault() {}});
  el.listeners.pointermove({target: a, clientX: 40, clientY: 40});
  flush();
  el.listeners.pointerup({target: a, clientX: 40, clientY: 40});
  ok("dragging a cell out grows its hive immediately",
     rect.getAttribute("x") === "0" && rect.getAttribute("y") === "-20",
     rect.getAttribute("x") + "," + rect.getAttribute("y"));
  sent.length = 0;

  const aBefore = boxAttr(a), bBefore = boxAttr(b);
  const wBefore = rect.getAttribute("width"), hBefore = rect.getAttribute("height");
  // The gesture starts on the hive's LABEL (GH #415): the fill pans, only the
  // label moves the group. The stub label answers `closest("[data-hive]")`
  // with its hive, exactly as the real DOM's containment does.
  label.closest = (sel) => (sel === "[data-hive]" ? hiveG : null);

  // A drag starting on the hive's FILL pans the camera and writes nothing —
  // that gesture once relocated an entire colony by accident.
  {
    const camBefore = vp.getAttribute("transform");
    el.listeners.pointerdown({target: hiveG, clientX: 0, clientY: 0, preventDefault() {}});
    el.listeners.pointermove({clientX: 30, clientY: 20});
    flush();
    el.listeners.pointerup({clientX: 30, clientY: 20});
    ok("a drag on a hive FILL pans instead of moving the group",
       vp.getAttribute("transform") !== camBefore && sent.length === 0,
       vp.getAttribute("transform") + " sent=" + JSON.stringify(sent));
    ok("and the members did not move",
       boxAttr(a).x === aBefore.x && boxAttr(a).y === aBefore.y,
       JSON.stringify(boxAttr(a)));
  }

  el.listeners.pointerdown({target: label, clientX: 0, clientY: 0, preventDefault() {}});
  el.listeners.pointermove({clientX: 120, clientY: 60});
  flush();
  el.listeners.pointerup({clientX: 120, clientY: 60});
  ok("the hive frame follows the drag",
     rect.getAttribute("x") === "120" && rect.getAttribute("y") === "40",
     rect.getAttribute("x") + "," + rect.getAttribute("y"));
  ok("and keeps its size — a move is not a resize",
     rect.getAttribute("width") === wBefore && rect.getAttribute("height") === hBefore,
     rect.getAttribute("width") + "x" + rect.getAttribute("height"));
  ok("its label follows too",
     label.getAttribute("x") === "128" && label.getAttribute("y") === "58",
     label.getAttribute("x") + "," + label.getAttribute("y"));
  ok("and both members move by the same delta",
     boxAttr(a).x === aBefore.x + 120 && boxAttr(a).y === aBefore.y + 60 &&
     boxAttr(b).x === bBefore.x + 120 && boxAttr(b).y === bBefore.y + 60,
     JSON.stringify([boxAttr(a), boxAttr(b)]));
  ok("a hive drag patches its MEMBERS, two props each, and nothing else",
     sent.length === 4 && sent.every(s => s.name === "object:set"),
     JSON.stringify(sent));
  ok("naming the member objects, at the positions they now hold",
     sent[0].payload.id === "n/a/one" && sent[0].payload.value === aBefore.x + 120 &&
     sent[1].payload.id === "n/a/one" && sent[1].payload.value === aBefore.y + 60 &&
     sent[2].payload.id === "n/a/two" && sent[2].payload.value === bBefore.x + 120 &&
     sent[3].payload.id === "n/a/two" && sent[3].payload.value === bBefore.y + 60,
     JSON.stringify(sent));
  sent.length = 0;

  // CTRL-DRAG PANS, whatever is under the cursor. On a dense arrangement there is
  // barely any empty background left to grab, and the denser it gets — which is
  // the direction arranging goes — the worse that gets.
  {
    const camBefore = vp.getAttribute("transform");
    const posBefore = boxAttr(a);
    el.listeners.pointerdown({target: a, clientX: 500, clientY: 500, ctrlKey: true,
                              preventDefault() {}});
    el.listeners.pointermove({target: a, clientX: 560, clientY: 530});
    flush();
    el.listeners.pointerup({target: a, clientX: 560, clientY: 530});
    ok("ctrl-drag on a cell moves the canvas, not the cell",
       vp.getAttribute("transform") !== camBefore &&
       boxAttr(a).x === posBefore.x && boxAttr(a).y === posBefore.y,
       vp.getAttribute("transform") + " / " + JSON.stringify(boxAttr(a)));
    // ...and it is never written anywhere. The 1.x client debounced a
    // `camera:moved` write 400 ms after the hand came to rest, which cost a
    // store round trip for a gesture that means nothing to anybody else.
    tick();
    ok("and the view it left is told to nobody", sent.length === 0, JSON.stringify(sent));
  }

  // HIVE IN HIVE. A hive's frame is the frame around its whole SUBTREE, so a drag
  // has to take the nested frames and the deep cells with it — moving only the
  // direct children left the inner boxes behind for one round trip and then
  // snapped them, which is what the nesting looked like from the client side.
  const deepCell = elem({"data-node": "a/b/deep/four", "data-oid": "n/a/b/deep/four",
                         transform: "translate(50,50)"});
  const innerRect = elem({x: "10", y: "10", width: "100", height: "60"}, "rect");
  const innerText = elem({x: "18", y: "28"}, "text");
  const innerG = elem({"data-hive": "a/b"});
  innerG.classList = {add() {}, remove() {}};
  innerG.querySelector = (sel) => (sel === "rect" ? innerRect : sel === "text" ? innerText : null);
  const deepRect = elem({x: "20", y: "20", width: "60", height: "40"}, "rect");
  const deepText = elem({x: "28", y: "38"}, "text");
  const deepG = elem({"data-hive": "a/b/deep"});
  deepG.classList = {add() {}, remove() {}};
  deepG.querySelector = (sel) => (sel === "rect" ? deepRect : sel === "text" ? deepText : null);
  innerG.closest = function (sel) { return sel === "[data-hive]" ? this : null; };
  const nestedVp = elem({});
  const nested = {
    classList: {add() {}, remove() {}},
    attrs: {"data-nw": "150", "data-nh": "38", "data-pad-side": "24",
            "data-pad-top": "30", "data-pad-bot": "24", "data-nest": "18"},
    getAttribute(k) { return k in this.attrs ? this.attrs[k] : null; },
    listeners: {},
    addEventListener(n, f) { this.listeners[n] = f; },
    removeEventListener(n) { delete this.listeners[n]; },
    querySelector(sel) { return sel === "g.viewport" ? nestedVp : null; },
    querySelectorAll(sel) {
      if (sel === "[data-node]") return [deepCell];
      if (sel === "[data-hive]") return [innerG, deepG];
      if (sel === "path.edge") return [];
      if (sel.startsWith("[data-from=")) return [];
      return [];
    },
  };
  const hook2 = Object.create(Canvy);
  hook2.el = nested;
  const sent2 = [];
  hook2.pushEvent = (n, p) => sent2.push({name: n, payload: p});
  hook2.mounted();
  // Grabbed by its label (GH #415) — the fill would pan.
  const innerLabel = innerG.querySelector("text");
  innerLabel.closest = (sel) => (sel === "[data-hive]" ? innerG : null);
  nested.listeners.pointerdown({target: innerLabel, clientX: 0, clientY: 0, preventDefault() {}});
  nested.listeners.pointermove({clientX: 70, clientY: 30});
  flush();
  nested.listeners.pointerup({clientX: 70, clientY: 30});
  // Both frames are DERIVED from the one cell that moved: the deep frame is that
  // cell padded, and the frame above it is the deep frame grown by the nesting
  // inset. So this also pins the containment — an ancestor is strictly bigger
  // than its child, by the same constant, wherever the drag ends up.
  ok("the dragged hive's own frame moves",
     innerRect.getAttribute("x") === "78" && innerRect.getAttribute("y") === "32",
     innerRect.getAttribute("x") + "," + innerRect.getAttribute("y"));
  ok("a NESTED frame moves with it, one inset inside its parent",
     deepRect.getAttribute("x") === "96" && deepRect.getAttribute("y") === "50" &&
     +deepRect.getAttribute("x") - +innerRect.getAttribute("x") === 18,
     deepRect.getAttribute("x") + "," + deepRect.getAttribute("y"));
  ok("and a cell two levels down moves too",
     boxAttr(deepCell).x === 120 && boxAttr(deepCell).y === 80,
     JSON.stringify(boxAttr(deepCell)));
  ok("one member, so one patch pair for the whole gesture",
     sent2.length === 2 &&
     sent2.every(s => s.name === "object:set" && s.payload.id === "n/a/b/deep/four") &&
     sent2[0].payload.value === 120 && sent2[1].payload.value === 80,
     JSON.stringify(sent2));
}

// --- 9. selection: the panel, the dimming, and the walk
//
// The stylesheet has had `.sel`, `.dim` and `.hot` since the first version —
// copied from a working tool — and nothing ever set them. So the picture could be
// looked at and not READ: which edges belong to this cell, what is the condition
// on that one, where does this go. That is the whole difference between a diagram
// and something you can dissect a colony with.
console.log("\nselection");
{
  const Canvy = (global.SurfaceHooks || globalThis.SurfaceHooks).Canvy;
  function classList() {
    const set = {};
    return {
      add(c) { set[c] = true; },
      remove() { Array.prototype.forEach.call(arguments, c => delete set[c]); },
      toggle(c, on) { if (on) set[c] = true; else delete set[c]; },
      has(c) { return !!set[c]; },
    };
  }
  function elem(attrs, tag) {
    return {
      tag: tag || "g",
      attrs: Object.assign({}, attrs),
      getAttribute(k) { return k in this.attrs ? String(this.attrs[k]) : null; },
      setAttribute(k, v) { this.attrs[k] = v; },
      removeAttribute(k) { delete this.attrs[k]; },
      querySelector() { return null; },
      querySelectorAll() { return []; },
      closest() { return null; },
    };
  }
  const cellA = elem({"data-node": "a/one", "data-oid": "n/a/one",
                      transform: "translate(0,0)"});
  const cellB = elem({"data-node": "a/two", "data-oid": "n/a/two",
                      transform: "translate(300,0)"});
  const cellC = elem({"data-node": "b/far", "data-oid": "n/b/far",
                      transform: "translate(900,0)"});
  [cellA, cellB, cellC].forEach(c => { c.classList = classList(); });
  const e1 = elem({"data-edge": "e1", "data-from": "a/one", "data-to": "a/two",
                   "data-lane": "0", "data-cond": "hop.route == 'x'",
                   "data-mod": '{"set_context":{"k":"v"}}'}, "path");
  const e2 = elem({"data-edge": "e2", "data-from": "b/far", "data-to": "b/far",
                   "data-lane": "0", "data-cond": "", "data-mod": ""}, "path");
  [e1, e2].forEach(e => { e.classList = classList(); });
  const panel = elem({id: "detail"});
  panel.innerHTML = "";
  panel.querySelectorAll = () => [];
  const vp2 = elem({});
  const board = {
    classList: classList(),
    attrs: {"data-nw": "150", "data-nh": "38", "data-pad-side": "24",
            "data-pad-top": "30", "data-pad-bot": "24", "data-nest": "18"},
    getAttribute(k) { return k in this.attrs ? this.attrs[k] : null; },
    listeners: {},
    addEventListener(n, f) { this.listeners[n] = f; },
    removeEventListener(n) { delete this.listeners[n]; },
    querySelector(sel) { return sel === "g.viewport" ? vp2 : sel === "#detail" ? panel : null; },
    querySelectorAll(sel) {
      if (sel === "[data-node]") return [cellA, cellB, cellC];
      if (sel === "path.edge") return [e1, e2];
      if (sel === "[data-hive]") return [];
      if (sel.startsWith("[data-from=")) return [];
      return [];
    },
  };
  // Escape is bound on the document, so this section needs its own.
  const keys = {};
  global.document = {
    addEventListener(n, f) { keys[n] = f; },
    removeEventListener(n) { delete keys[n]; },
  };
  const hook3 = Object.create(Canvy);
  hook3.el = board;
  hook3.pushEvent = () => {};
  hook3.mounted();

  cellA.closest = (sel) => (sel === "[data-node]" ? cellA : null);
  board.listeners.click({target: cellA});
  ok("selecting a cell marks it", cellA.classList.has("sel"));
  ok("its neighbour is not dimmed", !cellB.classList.has("dim"));
  ok("an unrelated cell is dimmed", cellC.classList.has("dim"));
  ok("its edge is hot", e1.classList.has("hot") && !e1.classList.has("dim"));
  ok("an unrelated edge is dimmed", e2.classList.has("dim"));
  ok("the panel names the cell and counts both directions",
     panel.innerHTML.indexOf("a/one") >= 0 &&
     panel.innerHTML.indexOf("out (1)") >= 0 &&
     panel.innerHTML.indexOf("in (0)") >= 0,
     panel.innerHTML.slice(0, 160));

  e1.closest = (sel) => (sel === "[data-edge]" ? e1 : null);
  board.listeners.click({target: e1});
  ok("selecting an edge shows its condition IN FULL",
     panel.innerHTML.indexOf("hop.route == &#39;x&#39;") >= 0 ||
     panel.innerHTML.indexOf("hop.route == 'x'") >= 0,
     panel.innerHTML.slice(0, 200));
  ok("and its modifier", panel.innerHTML.indexOf("set_context") >= 0);

  const empty = elem({});
  empty.closest = () => null;
  board.listeners.click({target: empty});
  ok("clicking the background clears everything",
     !cellA.classList.has("sel") && !e1.classList.has("hot") &&
     panel.innerHTML.indexOf("Click a cell") >= 0);

  // Escape lets go, without hunting for empty canvas to click on. Same reflex as
  // every other editor, and on a picture this dense the background is the hardest
  // thing to hit.
  e1.closest = (sel) => (sel === "[data-edge]" ? e1 : null);
  board.listeners.click({target: e1});
  ok("an edge can be selected again", e1.classList.has("hot"));
  keys.keydown({key: "Escape"});
  ok("Escape drops the selection", !e1.classList.has("hot") && !e1.classList.has("dim"));
  ok("and empties the panel", panel.innerHTML.indexOf("Click a cell or an edge") >= 0,
     panel.innerHTML.slice(0, 80));
  ok("and it is forgotten, so the next diff does not bring it back",
     !hook3.sel, String(hook3.sel));
}

// ── An edge that addresses a HIVE ───────────────────────────────────────────
//
// The boundary rule (overview § The hive boundary) says a caller talks to a hive,
// not into it. The layout has always written such an edge into the picture; the
// 1.x client dropped it, because `data-to` named something no `[data-node]`
// answered to and an endpoint without a box gets no `d`. In a real colony that
// meant 44 edges into the void, and the picture claimed there were none.
{
  const Canvy = (global.SurfaceHooks || globalThis.SurfaceHooks).Canvy;
  function elem(attrs, tag) {
    return {
      tag: tag || "g",
      attrs: Object.assign({}, attrs),
      getAttribute(k) { return k in this.attrs ? String(this.attrs[k]) : null; },
      setAttribute(k, v) { this.attrs[k] = v; },
      removeAttribute(k) { delete this.attrs[k]; },
      closest(sel) {
        const m = /^\[([\w-]+)/.exec(sel);
        return m && m[1] in this.attrs ? this : null;
      },
    };
  }
  const cell = elem({"data-node": "a/one", "data-oid": "n/a/one",
                     transform: "translate(100,100)"});
  const rect = elem({x: "400", y: "80", width: "300", height: "160"}, "rect");
  const hiveG = elem({"data-hive": "b"});
  hiveG.querySelector = (sel) => (sel === "rect" ? rect : null);
  const toHive = elem({"data-from": "a/one", "data-to": "b", "data-lane": "0"}, "path");
  const fromHive = elem({"data-from": "b", "data-to": "a/one", "data-lane": "1"}, "path");
  const vp = elem({});
  const el = {
    classList: {add() {}, remove() {}},
    attrs: {"data-nw": "150", "data-nh": "38", "data-pad-side": "24",
            "data-pad-top": "30", "data-pad-bot": "24", "data-nest": "18"},
    getAttribute(k) { return k in this.attrs ? this.attrs[k] : null; },
    listeners: {},
    addEventListener(n, f) { this.listeners[n] = f; },
    removeEventListener(n) { delete this.listeners[n]; },
    querySelector(sel) { return sel === "g.viewport" ? vp : null; },
    querySelectorAll(sel) {
      if (sel === "[data-node]") return [cell];
      if (sel === "path.edge") return [toHive, fromHive];
      if (sel === "[data-hive]") return [hiveG];
      return [];
    },
  };
  const hook = Object.create(Canvy);
  hook.el = el;
  hook.pushEvent = () => {};
  hook.mounted();

  const d1 = toHive.getAttribute("d"), d2 = fromHive.getAttribute("d");
  ok("an edge TO a hive is drawn", !!d1 && /^M[-\d.]+,[-\d.]+/.test(d1), String(d1));
  ok("an edge FROM a hive is drawn", !!d2 && /^M[-\d.]+,[-\d.]+/.test(d2), String(d2));
  ok("and neither carries NaN", !/NaN/.test(String(d1) + String(d2)),
     String(d1) + " | " + String(d2));

  // It has to meet the FRAME. The hive spans x 400..700; an edge that treated it
  // as a 150-wide cell at its corner would land at 400..550 and cut the box.
  const xs = String(d1).match(/-?[\d.]+/g).map(Number).filter((_, i) => i % 2 === 0);
  ok("and it stops at the hive's own rectangle, not at a cell-sized ghost",
     Math.max.apply(null, xs) <= 700 && Math.max.apply(null, xs) >= 380,
     String(d1));
}

// ── …and it has to move with the drag ───────────────────────────────────────
//
// "no edges while moving": a cell inside a hive changes that hive's FRAME while
// it moves, and an edge that ends on the frame was not in the set the drag
// collected — that set was built from the dragged cell's own id. So the line
// stayed where the frame used to be and read as detached.
{
  const Canvy = (global.SurfaceHooks || globalThis.SurfaceHooks).Canvy;
  function elem(attrs, tag) {
    return {
      tag: tag || "g",
      attrs: Object.assign({}, attrs),
      getAttribute(k) { return k in this.attrs ? String(this.attrs[k]) : null; },
      setAttribute(k, v) { this.attrs[k] = v; },
      removeAttribute(k) { delete this.attrs[k]; },
      closest(sel) {
        const m = /^\[([\w-]+)/.exec(sel);
        return m && m[1] in this.attrs ? this : null;
      },
    };
  }
  const raf2 = [];
  global.requestAnimationFrame = (f) => { raf2.push(f); return raf2.length; };
  const inner = elem({"data-node": "b/one", "data-oid": "n/b/one",
                      transform: "translate(420,100)"});
  const rect = elem({x: "400", y: "80", width: "300", height: "160"}, "rect");
  const label = elem({}, "text");
  const hiveG = elem({"data-hive": "b"});
  hiveG.querySelector = (sel) => (sel === "rect" ? rect : sel === "text" ? label : null);
  hiveG.closest = function (sel) { return sel === "[data-hive]" ? this : null; };
  const far = elem({"data-node": "a/one", "data-oid": "n/a/one",
                    transform: "translate(100,100)"});
  // The edge under test ends on the HIVE, not on the cell being dragged.
  const onFrame = elem({"data-from": "a/one", "data-to": "b", "data-lane": "0"}, "path");
  const vp = elem({});
  const el = {
    classList: {add() {}, remove() {}},
    attrs: {"data-nw": "150", "data-nh": "38", "data-pad-side": "24",
            "data-pad-top": "30", "data-pad-bot": "24", "data-nest": "18"},
    getAttribute(k) { return k in this.attrs ? this.attrs[k] : null; },
    listeners: {},
    addEventListener(n, f) { this.listeners[n] = f; },
    removeEventListener(n) { delete this.listeners[n]; },
    querySelector(sel) { return sel === "g.viewport" ? vp : null; },
    querySelectorAll(sel) {
      if (sel === "[data-node]") return [inner, far];
      if (sel === "path.edge") return [onFrame];
      if (sel === "[data-hive]") return [hiveG];
      return [];
    },
  };
  const hook = Object.create(Canvy);
  hook.el = el;
  hook.pushEvent = () => {};
  hook.mounted();
  const before = onFrame.getAttribute("d");
  ok("the frame edge starts out drawn", !!before, String(before));

  inner.closest = (sel) => (sel === "[data-node]" ? inner : null);
  el.listeners.pointerdown({target: inner, clientX: 0, clientY: 0,
                            button: 0, preventDefault() {}});
  el.listeners.pointermove({clientX: 260, clientY: 140, preventDefault() {}});
  raf2.splice(0).forEach(f => f());

  ok("dragging a cell moves the edge that ends on its hive's frame",
     onFrame.getAttribute("d") !== before,
     "before " + before + " now " + onFrame.getAttribute("d"));
  ok("and the moved edge carries no NaN",
     !/NaN/.test(String(onFrame.getAttribute("d"))),
     String(onFrame.getAttribute("d")));
}

// ── A door edge: one box INSIDE the other ───────────────────────────────────
//
// The shape the boundary rule produces (overview § The hive boundary): `.` -> the
// cell that serves the door. Two real rectangles, but nested, and "which side of
// the frame faces the cell" has no answer — every side does. The old router
// asked `side()` anyway and got the frame's OUTWARD normal, so the line left the
// hive through its outer wall, ran around the outside and came back in. All nine
// door edges in the live colony were drawn that way.
{
  const frame = {x: 100, y: 100, w: 400, h: 300};
  const cases = [
    ["cell near the top",    {x: 300, y: 130}],
    ["cell near the bottom", {x: 300, y: 340}],
    ["cell near the left",   {x: 120, y: 240}],
    ["cell near the right",  {x: 330, y: 240}],
    ["cell right at a wall", {x: 300, y: 108}],   // gap smaller than one stub
  ];
  for (const [name, cell] of cases) {
    for (const [dir, a, b] of [["out", frame, cell], ["in", cell, frame]]) {
      const r = G.route(a, b, W, H, 0);
      const pts = points(r.d);
      const out = pts.filter(p => p.x < frame.x - 0.6 || p.y < frame.y - 0.6 ||
                                  p.x > frame.x + frame.w + 0.6 ||
                                  p.y > frame.y + frame.h + 0.6);
      ok(`stays inside the frame (${name}, ${dir})`, out.length === 0, r.d);
      // …and it goes straight there. A path much longer than the gap it spans is
      // the detour this fix removes, whichever shape the detour happens to take.
      let len = 0;
      for (let i = 1; i < pts.length; i++) {
        len += Math.abs(pts[i].x - pts[i - 1].x) + Math.abs(pts[i].y - pts[i - 1].y);
      }
      const span = Math.abs(r.end.x - r.start.x) + Math.abs(r.end.y - r.start.y);
      ok(`no detour (${name}, ${dir})`, len <= Math.max(span, 8) * 1.35 + 1,
         `${len.toFixed(1)} for a span of ${span.toFixed(1)} — ${r.d}`);
      ok(`no NaN (${name}, ${dir})`, !/NaN/.test(r.d), r.d);
    }
  }

  // Two cells at the very same spot overlap, they are not nested — the equal-size
  // guard in `contains`. Without it the identical-box case above would be routed
  // as a door edge and collapse to a zero-length line.
  ok("identical boxes are not treated as nested",
     !G.contains({x: 0, y: 0}, {x: 0, y: 0}, W, H));
  ok("but a frame around a cell is",
     G.contains(frame, {x: 300, y: 130}, W, H));

  // The wall chosen is the NEAREST one: a cell hard against the left wall is met
  // from the left, not from wherever the centres happen to average out.
  const left = G.innerSide(frame, {x: 108, y: 240}, W, H);
  ok("the nearest wall is the door", left.x === -1 && left.y === 0,
     JSON.stringify(left));
  const bottom = G.innerSide(frame, {x: 300, y: 356}, W, H);
  ok("…on whichever side that is", bottom.x === 0 && bottom.y === 1,
     JSON.stringify(bottom));
}

console.log(fails ? `\n${fails} failing` : "\nall green");
process.exit(fails ? 1 : 0);
