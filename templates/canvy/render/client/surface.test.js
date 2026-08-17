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

console.log(fails ? `\n${fails} failing` : "\nall green");
process.exit(fails ? 1 : 0);
