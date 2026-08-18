// Edge routing for the topology view — the part worth testing separately.
//
// The first version drew a straight line between two box centres and trimmed
// the ends with `Math.min` over the two axis ratios. That is not the boundary
// of a rectangle, so arrowheads landed inside the box and lines crossed
// straight through cells that happened to sit between the endpoints. A picture
// whose lines run under the boxes is a picture you cannot follow with your eye,
// which was the whole complaint about the earlier attempts.
//
// So: leave a box through the side that faces the target, orthogonally, and
// approach the target the same way. Same idea as draw.io's orthogonal router,
// minus the obstacle avoidance — with a stub on each end, a line no longer
// starts under its own source, which is what actually made it unreadable.
//
// Loaded as a plain script (`window.TopoGeom`) and required by the test. The
// LiveView hook at the bottom of this file is its only browser consumer.

(function (root) {
  "use strict";

  const STUB = 18;      // how far a line runs straight out of a box before turning
  const LANE = 7;       // parallel edges between the same pair get separated by this
  const MARGIN = 26;    // how far outside everything the outermost free lane runs

  /// Which side of the box faces the target, as a unit vector.
  ///
  /// Decided by comparing the gap on each axis against the box's own aspect
  /// ratio, so a wide flat box prefers its top and bottom rather than its
  /// narrow sides whenever the target is even slightly above or below.
  function side(a, b, w, h) {
    // A box may carry its OWN size. A cell is 150x38 and every one of them is;
    // a HIVE is whatever its contents make it, and an edge that addresses a hive
    // (the boundary rule — overview § Die Hive-Grenze) has to leave and land on
    // that rectangle, not on a cell-sized ghost at its corner.
    const aw = a.w || w, ah = a.h || h, bw = b.w || w, bh = b.h || h;
    const dx = (b.x + bw / 2) - (a.x + aw / 2);
    const dy = (b.y + bh / 2) - (a.y + ah / 2);
    // Compared against the AVERAGE half-extent, so the choice of side does not
    // tip merely because one endpoint is a large rectangle.
    const mw = (aw + bw) / 2, mh = (ah + bh) / 2;
    if (Math.abs(dx) * mh > Math.abs(dy) * mw) {
      return dx >= 0 ? {x: 1, y: 0} : {x: -1, y: 0};
    }
    return dy >= 0 ? {x: 0, y: 1} : {x: 0, y: -1};
  }

  /// The point where a side's outward normal leaves the box.
  function anchor(box, dir, w, h, lane) {
    const bw = box.w || w, bh = box.h || h;
    const cx = box.x + bw / 2, cy = box.y + bh / 2;
    // The lane offset slides the anchor ALONG the chosen side, so two edges
    // between the same pair of cells do not overlap into one line.
    const along = dir.x === 0 ? {x: 1, y: 0} : {x: 0, y: 1};
    const limit = dir.x === 0 ? bw / 2 - 12 : bh / 2 - 8;
    const off = Math.max(-limit, Math.min(limit, lane * LANE));
    return {
      x: cx + dir.x * (bw / 2) + along.x * off,
      y: cy + dir.y * (bh / 2) + along.y * off,
    };
  }

  /// Does `a` wholly contain `b`?
  ///
  /// The case the hive boundary creates. A door edge (`{"from": "."}` — overview
  /// § Die Hive-Grenze) runs between a hive and a cell INSIDE it: two real boxes,
  /// but not two boxes side by side. "Which side of `a` faces `b`" has no answer
  /// when every side of the frame faces the cell, and `side()` answered it anyway
  /// — with the OUTWARD normal. So the line left the frame through its outer
  /// wall, ran around the outside and came back in. All nine door edges in the
  /// live colony were drawn that way.
  ///
  /// The equal-size case is excluded on purpose: two cells at the same spot
  /// contain each other by this test, and they are not nested, they overlap.
  function contains(a, b, w, h) {
    const aw = a.w || w, ah = a.h || h, bw = b.w || w, bh = b.h || h;
    return a.x <= b.x && a.y <= b.y &&
           a.x + aw >= b.x + bw && a.y + ah >= b.y + bh &&
           (aw > bw || ah > bh);
  }

  /// Which wall of `outer` a contained `inner` is met through, as that wall's
  /// OUTWARD normal: the nearest one. Nearest is both the shortest way in and
  /// the one that reads as "the door is on that side" — a hive's own frame is
  /// where a caller's eye arrives, so the line should enter where it lands.
  function innerSide(outer, inner, w, h) {
    const ow = outer.w || w, oh = outer.h || h;
    const iw = inner.w || w, ih = inner.h || h;
    const gaps = [
      [inner.x - outer.x, {x: -1, y: 0}],
      [(outer.x + ow) - (inner.x + iw), {x: 1, y: 0}],
      [inner.y - outer.y, {x: 0, y: -1}],
      [(outer.y + oh) - (inner.y + ih), {x: 0, y: 1}],
    ];
    gaps.sort((p, q) => p[0] - q[0]);
    return gaps[0][1];
  }

  /// The point on `dir`'s wall of `box` that lies opposite `toward`'s centre.
  ///
  /// `anchor` puts the point in the MIDDLE of the wall, which is right when the
  /// two boxes face each other and wrong when one is a frame around the other: a
  /// hive is metres wide, and leaving from the middle of its top wall to reach a
  /// cell at its right edge draws a dog-leg where a straight line belongs. Both
  /// ends of a door edge line up on the inner box, so the line goes straight in.
  function faceAt(box, dir, w, h, toward, lane) {
    const bw = box.w || w, bh = box.h || h;
    const tw = toward.w || w, th = toward.h || h;
    const cx = box.x + bw / 2, cy = box.y + bh / 2;
    const off = (lane || 0) * LANE;
    if (dir.x === 0) {
      const t = toward.x + tw / 2 + off;
      return {x: Math.max(box.x + 12, Math.min(box.x + bw - 12, t)),
              y: cy + dir.y * (bh / 2)};
    }
    const t = toward.y + th / 2 + off;
    return {x: cx + dir.x * (bw / 2),
            y: Math.max(box.y + 8, Math.min(box.y + bh - 8, t))};
  }

  /// Where a route leaves each box, and in which direction it travels from there.
  ///
  /// Two different things, and they only coincide when the boxes stand apart: a
  /// frame's anchor sits on its own wall (normal pointing OUT) while the line
  /// from it travels IN. Keeping the anchor normal and the travel direction as
  /// separate values is the whole fix — the old code used one vector for both,
  /// which is exactly why a door edge was drawn inside out.
  function ends(a, b, w, h, lane) {
    if (contains(a, b, w, h)) {
      const n = innerSide(a, b, w, h);
      return {p0: faceAt(a, n, w, h, b, lane), p3: faceAt(b, n, w, h, b, lane),
              ta: {x: -n.x, y: -n.y}, tb: n, nested: true};
    }
    if (contains(b, a, w, h)) {
      const n = innerSide(b, a, w, h);
      return {p0: faceAt(a, n, w, h, a, lane), p3: faceAt(b, n, w, h, a, lane),
              ta: n, tb: {x: -n.x, y: -n.y}, nested: true};
    }
    const da = side(a, b, w, h);
    const db = {x: -da.x, y: -da.y};
    return {p0: anchor(a, da, w, h, lane), p3: anchor(b, db, w, h, lane),
            ta: da, tb: db, nested: false};
  }

  /// An orthogonal path from box `a` to box `b`, as an SVG path string.
  ///
  /// `lane` separates parallel edges; pass the index among edges sharing this
  /// pair, centred on zero.
  function route(a, b, w, h, lane, lanes) {
    lane = lane || 0;
    const {p0, p3, ta, tb, nested} = ends(a, b, w, h, lane);
    // Inside a frame the two stubs point AT each other, so a stub longer than
    // half the gap overshoots and the line doubles back on itself. A cell sitting
    // 20 px below its hive's wall gets a short stub rather than a knot.
    const reach = nested
      ? Math.max(2, Math.min(STUB, (Math.abs(ta.x ? p3.x - p0.x : p3.y - p0.y)) / 2 - 1))
      : STUB;
    const p1 = {x: p0.x + ta.x * reach, y: p0.y + ta.y * reach};
    const p2 = {x: p3.x + tb.x * reach, y: p3.y + tb.y * reach};

    // Between the two stubs, turn at most twice. Horizontal exit means travel
    // horizontally first; vertical exit means vertically first.
    //
    // WHERE it turns is the whole difference between a diagram and a thicket.
    // Turning at the midpoint puts the crossing wherever the two boxes happen to
    // average out, which on a real colony ran 47% of the lines straight through
    // cells they had nothing to do with. `lanes` offers the empty corridors
    // BETWEEN the columns — the gaps the layout leaves on purpose — and the turn
    // goes into the nearest one that actually lies on the way.
    const horizontal = ta.x !== 0;
    const bend = (turn) => (horizontal
      ? [{x: p1.x, y: p1.y}, {x: turn, y: p1.y},
         {x: turn, y: p2.y}, {x: p2.x, y: p2.y}]
      : [{x: p1.x, y: p1.y}, {x: p1.x, y: turn},
         {x: p2.x, y: turn}, {x: p2.x, y: p2.y}]);

    const from = horizontal ? p1.x : p1.y;
    const to = horizontal ? p2.x : p2.y;
    const offered = (lanes && (horizontal ? lanes.x : lanes.y)) || [];
    const candidates = [corridor(offered, from, to)].concat(
      offered.filter(v => v > Math.min(from, to) && v < Math.max(from, to))
        .sort((u, v) => Math.abs(u - (from + to) / 2) - Math.abs(v - (from + to) / 2)));

    // Try the corridors in order and take the first that touches nothing. The
    // obstacles are known — they are the boxes on screen — so "does this line run
    // through a cell" is a question with an answer, not a guess. Falls back to the
    // fewest crossings when every candidate hits something, because a line has to
    // be drawn either way.
    const boxes = (lanes && lanes.boxes) || null;
    let best = null, bestHits = Infinity;
    for (let i = 0; i < candidates.length; i++) {
      const hits = crossings([p0].concat(bend(candidates[i]), [p3]), boxes, w, h, a, b);
      if (hits === 0) { best = bend(candidates[i]); bestHits = 0; break; }
      if (hits < bestHits) { bestHits = hits; best = bend(candidates[i]); }
    }

    // Still blocked? Then a box sits on the line the anchors leave along, and no
    // choice of turning point can help — the way past it is to step aside FIRST.
    // One more bend, offered only when the simple path fails: a picture where
    // every line zigzags is as hard to read as one where the lines run under the
    // boxes, so the plain route keeps its right of way.
    // …but never for a door edge. Stepping aside there means stepping OUT of the
    // frame the edge is drawn inside, which is the very picture this fix removes.
    // A door edge that clips a sibling cell is a layout complaint, not a routing
    // one — the honest short line stays.
    if (bestHits > 0 && !nested) {
      const across = (lanes && (horizontal ? lanes.y : lanes.x)) || [];
      const near = horizontal ? p1.y : p1.x;
      const sorted = across.slice().sort(
        (u, v) => Math.abs(u - near) - Math.abs(v - near));
      for (let i = 0; i < sorted.length; i++) {
        const step = horizontal
          ? [{x: p1.x, y: p1.y}, {x: p1.x, y: sorted[i]},
             {x: p2.x, y: sorted[i]}, {x: p2.x, y: p2.y}]
          : [{x: p1.x, y: p1.y}, {x: sorted[i], y: p1.y},
             {x: sorted[i], y: p2.y}, {x: p2.x, y: p2.y}];
        if (crossings([p0].concat(step, [p3]), boxes, w, h, a, b) === 0) {
          best = step;
          break;
        }
      }
    }

    const pts = [p0].concat(best, [p3]);
    return {d: rounded(pts), start: p0, end: p3};
  }

  /// How many boxes an orthogonal polyline runs through, ignoring its own two
  /// endpoints' boxes.
  ///
  /// Every segment here is axis-parallel, so this is interval arithmetic and not
  /// sampling: cheap enough to ask once per candidate per edge, which is what
  /// makes choosing a clear route affordable at all.
  function crossings(pts, boxes, w, h, skipA, skipB) {
    if (!boxes || !boxes.length) return 0;
    let n = 0;
    for (let b = 0; b < boxes.length; b++) {
      const box = boxes[b];
      if (skipA && box.x === skipA.x && box.y === skipA.y) continue;
      if (skipB && box.x === skipB.x && box.y === skipB.y) continue;
      const bx1 = box.x, bx2 = box.x + w, by1 = box.y, by2 = box.y + h;
      for (let i = 1; i < pts.length; i++) {
        const p = pts[i - 1], q = pts[i];
        const lox = Math.min(p.x, q.x), hix = Math.max(p.x, q.x);
        const loy = Math.min(p.y, q.y), hiy = Math.max(p.y, q.y);
        if (hix > bx1 + 0.6 && lox < bx2 - 0.6 && hiy > by1 + 0.6 && loy < by2 - 0.6) {
          n++;
          break;
        }
      }
    }
    return n;
  }

  /// The turning point between `from` and `to`: the free corridor closest to the
  /// midpoint, or the midpoint itself when none lies between them. Detouring to a
  /// corridor OUTSIDE the span would trade a crossing for a longer line, which is
  /// not a trade worth making.
  function corridor(list, from, to) {
    const mid = (from + to) / 2;
    if (!list || !list.length) return mid;
    const lo = Math.min(from, to), hi = Math.max(from, to);
    let best = null, bestGap = Infinity;
    for (let i = 0; i < list.length; i++) {
      const v = list[i];
      if (v <= lo || v >= hi) continue;
      const gap = Math.abs(v - mid);
      if (gap < bestGap) { bestGap = gap; best = v; }
    }
    return best === null ? mid : best;
  }

  /// The empty bands between the boxes, on each axis.
  ///
  /// Every box occupies an interval; merge them and what is left between two
  /// merged runs is a lane no box stands in. The layout leaves these gaps
  /// deliberately — they are the space between two columns — so routing through
  /// them is using the arrangement rather than fighting it.
  function freeLanes(boxes, w, h) {
    const bands = (lo, size) => {
      const iv = boxes.map(b => [lo(b), lo(b) + size]).sort((p, q) => p[0] - q[0]);
      const merged = [];
      for (const [s, e] of iv) {
        const last = merged[merged.length - 1];
        if (last && s <= last[1]) last[1] = Math.max(last[1], e);
        else merged.push([s, e]);
      }
      const out = [];
      for (let i = 1; i < merged.length; i++) {
        out.push((merged[i - 1][1] + merged[i][0]) / 2);
      }
      // Plus the open ground on either side of everything. Without it a row of
      // boxes all at the same height offers no lane at all — and that is exactly
      // the arrangement a flow layout produces, so the one case with no gap
      // between obstacles is also the most common one.
      if (merged.length) {
        out.push(merged[0][0] - MARGIN);
        out.push(merged[merged.length - 1][1] + MARGIN);
      }
      return out;
    };
    // The boxes ride along so the router can ask whether a candidate is clear.
    return {x: bands(b => b.x, w), y: bands(b => b.y, h), boxes: boxes};
  }

  /// A polyline with rounded corners, so the eye follows a turn instead of
  /// stopping at it.
  function rounded(pts, r) {
    r = r === undefined ? 6 : r;
    const out = [`M${fmt(pts[0].x)},${fmt(pts[0].y)}`];
    for (let i = 1; i < pts.length - 1; i++) {
      const p = pts[i], prev = pts[i - 1], next = pts[i + 1];
      const inLen = Math.hypot(p.x - prev.x, p.y - prev.y);
      const outLen = Math.hypot(next.x - p.x, next.y - p.y);
      if (inLen < 0.5 || outLen < 0.5) continue;           // degenerate corner
      const rr = Math.min(r, inLen / 2, outLen / 2);
      const a = {x: p.x - (p.x - prev.x) / inLen * rr, y: p.y - (p.y - prev.y) / inLen * rr};
      const b = {x: p.x + (next.x - p.x) / outLen * rr, y: p.y + (next.y - p.y) / outLen * rr};
      out.push(`L${fmt(a.x)},${fmt(a.y)}`);
      if (rr > 0.5) out.push(`Q${fmt(p.x)},${fmt(p.y)} ${fmt(b.x)},${fmt(b.y)}`);
    }
    const last = pts[pts.length - 1];
    out.push(`L${fmt(last.x)},${fmt(last.y)}`);
    return out.join(" ");
  }

  function fmt(n) {
    return Math.round(n * 10) / 10;
  }

  /// Does a segment pass through a box? Used by the test to prove the routing
  /// keeps clear of its own endpoints' boxes.
  function segmentHitsBox(p, q, box, w, h) {
    const x1 = box.x, y1 = box.y, x2 = box.x + w, y2 = box.y + h;
    for (let t = 0; t <= 1.0001; t += 0.02) {
      const x = p.x + (q.x - p.x) * t, y = p.y + (q.y - p.y) * t;
      if (x > x1 + 0.6 && x < x2 - 0.6 && y > y1 + 0.6 && y < y2 - 0.6) return true;
    }
    return false;
  }

  /// The finished `d` attribute for one edge — the one call the hook makes.
  ///
  /// It exists because the hook used to say `rounded(route(...))`, and `route`
  /// returns `{d, start, end}` while `rounded` takes an array of points: the
  /// result was `MNaN,NaN`, i.e. an invisible line. Every property test below
  /// passed, because they all used `route(...).d` — the defect lived in the ONE
  /// expression no test evaluated. Now there is only one way to spell it.
  function edgePath(a, b, w, h, lane, lanes) {
    return route(a, b, w, h, lane, lanes).d;
  }

  const api = {STUB, LANE, side, anchor, route, rounded, edgePath, segmentHitsBox,
               freeLanes, corridor, crossings, contains, innerSide, faceAt, ends};
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  root.TopoGeom = api;
})(typeof window !== "undefined" ? window : globalThis);

// ---------------------------------------------------------------------------
// The LiveView hook. Two jobs, both purely presentational.
//
// # 1. Draw the edges
//
// The server sends each edge as its ENDPOINTS and a lane number, never as a
// finished path: the orthogonal routing above is presentation, and computing a
// `d` on both sides would be one algorithm in two languages. So every time the
// server's markup lands, this walks the edges and fills in their `d`.
//
// # 2. Own the drag
//
// Between pointerdown and pointerup this moves a box and the lines attached to
// it, and that is all. It holds no state that outlives the drag, it writes to
// nothing, and it never decides where a box belongs — on release it says "the
// user let go at 700,240" and the server answers with where the box IS. The
// client is never right, it is only faster.
//
// The server does not see the movement. Ruling 2026-08-17: start and end
// events only. Twenty renders a second would spend a third of a core (two code
// cell calls, ~34 ms) redrawing something nobody looks at until they let go.
// Two events per drag cost nothing, and the diff on release is authoritative.
//
// The lines move WITH their box during a drag, as straight rubber bands from
// anchor to anchor — deliberately NOT the orthogonal routing, which is
// recomputed when the diff lands. A box that detaches from its lines while
// moving was the reported defect of the first version of this view; a rubber
// band that snaps to the real route on release is honest about being
// provisional.
//
// No `phx-update="ignore"` anywhere: the server renders everything and the diff
// must win. The provisional DOM is overwritten on purpose — that is the whole
// SSR statement.
(function (root) {
  "use strict";
  if (typeof document === "undefined") return;   // required by the test, not run

  const G = root.TopoGeom;
  const NODE_W = 150, NODE_H = 38;

  function boxOf(g) {
    const m = /translate\((-?[\d.]+),\s*(-?[\d.]+)\)/.exec(
      g.getAttribute("transform") || "");
    return m ? {x: +m[1], y: +m[2]} : {x: 0, y: 0};
  }

  function centre(box) {
    return {x: box.x + NODE_W / 2, y: box.y + NODE_H / 2};
  }

  /// Fill in every edge's `d` from the endpoints the server named.
  /// Every endpoint an edge can name, as a box: the cells, and — since the
  /// boundary rule — the hive frames. `cellsOnly` is what the corridor search
  /// gets, because a hive is a container and never an obstacle.
  function boxMap(el, cellsOnly) {
    const boxes = {};
    el.querySelectorAll("[data-node]").forEach(function (g) {
      boxes[g.getAttribute("data-node")] = boxOf(g);
    });
    if (cellsOnly) return boxes;
    el.querySelectorAll("[data-hive]").forEach(function (g) {
      const id = g.getAttribute("data-hive");
      const r = g.querySelector && g.querySelector("rect");
      if (!id || !r || boxes[id]) return;
      boxes[id] = {
        x: +(r.getAttribute("x") || 0), y: +(r.getAttribute("y") || 0),
        w: +(r.getAttribute("width") || 0), h: +(r.getAttribute("height") || 0),
      };
    });
    return boxes;
  }

  /// The edges a drag has to move: the ones attached to what is being dragged,
  /// plus the ones attached to every hive ABOVE it. A cell inside a hive changes
  /// that hive's frame, and an edge that ends on the frame has to follow — or it
  /// stays where the frame used to be, which is the "beim move sind keine kanten"
  /// report: the line does not vanish, it stops being attached to anything.
  function edgesFor(el, ids) {
    const want = {};
    ids.forEach(function (id) {
      want[id] = true;
      const parts = id.split("/");
      for (let i = 1; i < parts.length; i++) want[parts.slice(0, i).join("/")] = true;
    });
    return Array.from(el.querySelectorAll("path.edge")).filter(function (p) {
      return want[p.getAttribute("data-from")] || want[p.getAttribute("data-to")];
    });
  }

  function drawEdges(el) {
    const nodes = boxMap(el, true);
    // The cells are the obstacles — a hive is a container, not something a line
    // has to go around, and treating one as an obstacle would send every edge on
    // a detour around the box it is drawn inside.
    const lanes = G.freeLanes(Object.keys(nodes).map(k => nodes[k]), NODE_W, NODE_H);
    // Hives are endpoints too — added AFTER the corridors, so a frame is never
    // an obstacle, and with their own width and height so a line meets the
    // rectangle rather than a cell-sized ghost in its corner.
    const all = boxMap(el, false);
    Object.keys(all).forEach(function (k) { if (!nodes[k]) nodes[k] = all[k]; });
    el.querySelectorAll("path.edge").forEach(function (p) {
      const a = nodes[p.getAttribute("data-from")];
      const b = nodes[p.getAttribute("data-to")];
      if (!a || !b) { p.removeAttribute("d"); return; }
      const lane = parseInt(p.getAttribute("data-lane") || "0", 10);
      const d = G.edgePath(a, b, NODE_W, NODE_H, lane, lanes);
      p.setAttribute("d", d);
      // The fat invisible twin follows the same path — it is what a mouse hits.
      const hit = p.parentNode && p.parentNode.querySelector
        ? p.parentNode.querySelector("path.edge-hit") : null;
      if (hit) hit.setAttribute("d", d);
    });
  }

  /// Everything BELOW a hive: every cell at any depth, not just its direct
  /// children. A hive's frame is the frame around its whole subtree, so a drag
  /// that moved only the direct children would leave the nested hives behind for
  /// one round trip and then snap them — which is what "hive in hive funktioniert
  /// noch nicht richtig" looked like from the client side.
  function membersOf(el, hive) {
    const prefix = hive + "/";
    return Array.from(el.querySelectorAll("[data-node]")).filter(function (g) {
      return (g.getAttribute("data-node") || "").startsWith(prefix);
    });
  }

  /// A hive group's `<rect>` and `<text>`, so a drag can move the frame with its
  /// contents instead of leaving it behind for one round trip.
  function hiveParts(g) {
    return {rect: g.querySelector ? g.querySelector("rect") : null,
            text: g.querySelector ? g.querySelector("text") : null};
  }

  /// The layout constants, read from the markup the server rendered.
  ///
  /// Read and not repeated: the frames a drag computes have to be the frames the
  /// next diff brings back, and two copies of six numbers are two layouts that
  /// agree right up until somebody changes one of them.
  function geometryOf(el) {
    const n = (k, d) => {
      const v = parseFloat(el.getAttribute(k));
      return isFinite(v) ? v : d;
    };
    return {w: n("data-nw", NODE_W), h: n("data-nh", NODE_H),
            side: n("data-pad-side", 24), top: n("data-pad-top", 30),
            bot: n("data-pad-bot", 24), nest: n("data-nest", 18)};
  }

  /// Every hive's rectangle, recomputed from where the cells are RIGHT NOW.
  ///
  /// The same union the server computes: a hive's own cells padded, plus every
  /// child's frame grown by the nesting inset. Which is why dragging a cell out
  /// of a hive grows that hive AND every hive above it while the cursor is still
  /// down — the frames are derived, so they can be derived again at 60 Hz instead
  /// of waiting for a round trip. A frame that only updates on release is a frame
  /// that lies for as long as you are looking at it.
  function frameMap(el, geom) {
    const own = {}, kids = {}, seen = {};
    el.querySelectorAll("[data-node]").forEach(function (g) {
      const id = g.getAttribute("data-node") || "";
      const cut = id.lastIndexOf("/");
      const h = cut < 0 ? "" : id.slice(0, cut);
      (own[h] = own[h] || []).push(boxOf(g));
    });
    el.querySelectorAll("[data-hive]").forEach(function (g) {
      const h = g.getAttribute("data-hive") || "";
      seen[h] = true;
      const cut = h.lastIndexOf("/");
      const p = cut < 0 ? "" : h.slice(0, cut);
      (kids[p] = kids[p] || []).push(h);
    });
    const out = {};
    function rect(h) {
      if (out[h]) return out[h];
      const boxes = [];
      if (own[h] && own[h].length) {
        const xs = own[h].map(p => p.x), ys = own[h].map(p => p.y);
        const x = Math.min.apply(null, xs) - geom.side;
        const y = Math.min.apply(null, ys) - geom.top;
        boxes.push({x: x, y: y,
                    w: Math.max.apply(null, xs) + geom.w + geom.side - x,
                    h: Math.max.apply(null, ys) + geom.h + geom.bot - y});
      }
      (kids[h] || []).forEach(function (c) {
        const r = rect(c);
        if (r) boxes.push({x: r.x - geom.nest, y: r.y - geom.nest,
                           w: r.w + 2 * geom.nest, h: r.h + 2 * geom.nest});
      });
      if (!boxes.length) return null;
      const x = Math.min.apply(null, boxes.map(b => b.x));
      const y = Math.min.apply(null, boxes.map(b => b.y));
      out[h] = {x: x, y: y,
                w: Math.max.apply(null, boxes.map(b => b.x + b.w)) - x,
                h: Math.max.apply(null, boxes.map(b => b.y + b.h)) - y};
      return out[h];
    }
    Object.keys(seen).sort((a, b) => b.split("/").length - a.split("/").length)
      .forEach(rect);
    return out;
  }

  /// Redraw every hive rectangle from the current cell positions.
  function applyFrames(el, geom) {
    const map = frameMap(el, geom);
    el.querySelectorAll("[data-hive]").forEach(function (g) {
      const r = map[g.getAttribute("data-hive")];
      if (!r) return;
      const p = hiveParts(g);
      if (p.rect) {
        p.rect.setAttribute("x", Math.round(r.x));
        p.rect.setAttribute("y", Math.round(r.y));
        p.rect.setAttribute("width", Math.round(r.w));
        p.rect.setAttribute("height", Math.round(r.h));
      }
      if (p.text) {
        p.text.setAttribute("x", Math.round(r.x) + 8);
        p.text.setAttribute("y", Math.round(r.y) + 18);
      }
    });
  }

  function numAttr(el, k) {
    const v = parseFloat(el && el.getAttribute ? el.getAttribute(k) : NaN);
    return isFinite(v) ? v : 0;
  }

  /// The provisional line during a drag: straight, centre to centre.
  function rubberBand(p, a, b) {
    const ca = centre(a), cb = centre(b);
    p.setAttribute("d", "M" + ca.x + "," + ca.y + " L" + cb.x + "," + cb.y);
  }

  /// The `<g class="viewport">` the camera transform lives on.
  function viewportOf(el) {
    return el.querySelector("g.viewport");
  }

  /// Read the camera the SERVER rendered. The client starts from what the store
  /// holds, so a reload does not throw a view away.
  function cameraOf(el) {
    const g = viewportOf(el);
    if (!g) return {x: 0, y: 0, z: 1000};
    const n = (k, d) => {
      const v = parseFloat(g.getAttribute(k));
      return isFinite(v) ? v : d;
    };
    return {x: n("data-cx", 0), y: n("data-cy", 0), z: n("data-cz", 1000)};
  }

  function applyCamera(el, cam) {
    const g = viewportOf(el);
    if (!g) return;
    g.setAttribute("transform",
      "translate(" + Math.round(cam.x) + "," + Math.round(cam.y) + ") scale(" +
      (cam.z / 1000).toFixed(3) + ")");
  }

  /// A client point in the SVG's own user space — the space the boxes live in.
  ///
  /// Without this a drag moves the box by CSS pixels while the picture is scaled
  /// to fit its frame, so the box lags or overshoots the cursor by exactly the
  /// zoom factor. `getScreenCTM` is the only thing that knows the whole chain
  /// (viewBox fit x camera transform), so ask it rather than reconstruct it.
  function userPoint(el, ev) {
    const svg = el.querySelector("svg.stage");
    const g = viewportOf(el);
    if (!svg || !g || !svg.createSVGPoint || !g.getScreenCTM) {
      return {x: ev.clientX, y: ev.clientY};
    }
    const m = g.getScreenCTM();
    if (!m) return {x: ev.clientX, y: ev.clientY};
    const p = svg.createSVGPoint();
    p.x = ev.clientX;
    p.y = ev.clientY;
    const q = p.matrixTransform(m.inverse());
    return {x: q.x, y: q.y};
  }

  // ── selection ─────────────────────────────────────────────────────────────
  //
  // Click a cell: its edges light up, everything unrelated dims, and the panel
  // lists what points at it and where it points — each entry clickable, so a
  // colony can be walked one hop at a time. Click an edge: its condition and its
  // modifier in full, which is the thing you actually need when a message did not
  // go where you expected.
  //
  // Entirely client-side and deliberately so: what is selected is not a fact about
  // the colony, and a round trip per click would make reading a graph cost cell
  // calls. Everything it needs is already in the markup the server sent.

  function esc(v) {
    return String(v === undefined || v === null ? "" : v)
      .replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  }

  function edgesOf(el) {
    return Array.from(el.querySelectorAll("path.edge")).map(function (p) {
      return {
        p: p,
        id: p.getAttribute("data-edge"),
        from: p.getAttribute("data-from"),
        to: p.getAttribute("data-to"),
        cond: p.getAttribute("data-cond") || "",
        mod: p.getAttribute("data-mod") || "",
      };
    });
  }

  function clearSelection(el) {
    el.querySelectorAll("path.edge").forEach(function (p) {
      p.classList.remove("hot", "dim");
    });
    el.querySelectorAll("[data-node]").forEach(function (g) {
      g.classList.remove("dim", "sel");
    });
    const d = el.querySelector("#detail");
    if (d) d.innerHTML = '<p class="empty">Click a cell or an edge.</p>';
  }

  /// Fill the panel and dim what is not involved. `id` is a cell path or an edge
  /// id; anything else clears.
  function select(el, id) {
    const detail = el.querySelector("#detail");
    if (!detail) return;
    const edges = edgesOf(el);
    const cells = Array.from(el.querySelectorAll("[data-node]"));
    const edge = edges.find(e => e.id === id);
    const cellG = cells.find(g => g.getAttribute("data-node") === id);
    if (!edge && !cellG) { clearSelection(el); return; }

    if (edge) {
      edges.forEach(e => {
        e.p.classList.toggle("hot", e.id === id);
        e.p.classList.toggle("dim", e.id !== id);
      });
      cells.forEach(g => {
        const p = g.getAttribute("data-node");
        g.classList.toggle("dim", p !== edge.from && p !== edge.to);
        g.classList.remove("sel");
      });
      detail.innerHTML =
        '<dl class="kv"><dt>edge</dt><dd>' + esc(edge.from) + "<br>→ " + esc(edge.to) +
        '</dd></dl><dl class="kv"><dt>condition</dt><dd>' +
        (edge.cond ? esc(edge.cond) : '<span class="chip">unconditional</span>') +
        "</dd></dl>" +
        (edge.mod ? '<dl class="kv"><dt>modifier</dt><dd>' + esc(edge.mod) + "</dd></dl>" : "");
      return;
    }

    const mine = edges.filter(e => e.from === id || e.to === id);
    const near = {};
    near[id] = true;
    mine.forEach(e => { near[e.from] = true; near[e.to] = true; });
    edges.forEach(e => {
      const rel = mine.indexOf(e) >= 0;
      e.p.classList.toggle("hot", rel);
      e.p.classList.toggle("dim", !rel);
    });
    cells.forEach(g => {
      const p = g.getAttribute("data-node");
      g.classList.toggle("dim", !near[p]);
      g.classList.toggle("sel", p === id);
    });
    const ty = cellG.querySelector ? cellG.querySelector("text.ty") : null;
    const row = function (list, other) {
      if (!list.length) return '<span class="chip">none</span>';
      return list.map(e => '<div class="rel" data-rel="' + esc(e.id) + '">' +
        esc(other(e)) + (e.cond ? ' <span class="chip">if</span>' : "") + "</div>").join("");
    };
    const ins = mine.filter(e => e.to === id), outs = mine.filter(e => e.from === id);
    detail.innerHTML =
      '<dl class="kv"><dt>cell</dt><dd>' + esc(id) + "</dd></dl>" +
      '<dl class="kv"><dt>type</dt><dd>' + esc(ty ? ty.textContent : "") + "</dd></dl>" +
      '<dl class="kv"><dt>in (' + ins.length + ")</dt><dd>" + row(ins, e => e.from) + "</dd></dl>" +
      '<dl class="kv"><dt>out (' + outs.length + ")</dt><dd>" + row(outs, e => e.to) + "</dd></dl>";
    detail.querySelectorAll(".rel").forEach(function (n) {
      n.addEventListener("click", function () { select(el, n.getAttribute("data-rel")); });
    });
  }

  const Canvy = {
    mounted() {
      this.cam = cameraOf(this.el);
      this.geom = geometryOf(this.el);
      this.wire();
      applyCamera(this.el, this.cam);
      drawEdges(this.el);
    },
    // The server re-renders the whole tree, so its `transform` and its camera
    // attributes land again with every diff. Re-applying is not a workaround for
    // the SSR model, it IS the model: the client owns the view, the server owns
    // the picture.
    updated() {
      this.geom = geometryOf(this.el);
      applyCamera(this.el, this.cam);
      drawEdges(this.el);
      // The server re-renders the whole tree, so the selection's classes and the
      // panel are gone with every diff. Re-applying is the same move as the
      // camera: the client owns the view, the server owns the picture.
      if (this.sel) select(this.el, this.sel); else clearSelection(this.el);
    },
    destroyed() { this.unwire(); },

    wire() {
      const el = this.el, hook = this;
      let drag = null;
      let hive = null;
      let pan = null;
      let frame = null;

      // Remember where the operator is looking. Debounced: a wheel produces
      // dozens of events and each render is a full page across the wire, so the
      // camera is written once the hand comes to rest — 400 ms after the last
      // change, which is below noticing and far above one write per tick.
      this.saveCamera = function () {
        if (hook.camTimer) clearTimeout(hook.camTimer);
        hook.camTimer = setTimeout(function () {
          hook.camTimer = null;
          hook.pushEvent("camera:moved", {
            x: Math.round(hook.cam.x), y: Math.round(hook.cam.y),
            z: Math.round(hook.cam.z),
          });
        }, 400);
      };

      // Zoom around the cursor: the point under the pointer stays under it, which
      // is the only zoom that does not feel like being teleported.
      this.onWheel = function (ev) {
        ev.preventDefault();
        const before = userPoint(el, ev);
        const factor = Math.exp(-ev.deltaY / 400);
        const z = Math.max(100, Math.min(4000, hook.cam.z * factor));
        hook.cam.z = z;
        applyCamera(el, hook.cam);
        const after = userPoint(el, ev);
        hook.cam.x += (after.x - before.x) * (z / 1000);
        hook.cam.y += (after.y - before.y) * (z / 1000);
        applyCamera(el, hook.cam);
        hook.saveCamera();
      };

      // A click that did not drag is a selection. Decided on pointerUP by whether
      // the pointer moved, so a drag never selects and a click never has to be
      // held still — the same rule the reference viewer uses.
      this.onClick = function (ev) {
        if (hook.dragged) { hook.dragged = false; return; }
        const t = ev.target;
        // The one control in the chrome. The server offers it only when the table
        // holds rows naming a cell or a hive the colony no longer has, and it is a
        // press rather than a housekeeping pass because a rename and a removal are
        // indistinguishable from the table's side — the operator is the only party
        // who knows which happened (GH #184).
        if (t.closest && t.closest("[data-sweep]")) {
          hook.pushEvent("canvas:sweep", {});
          return;
        }
        const edge = t.closest ? t.closest("[data-edge]") : null;
        const node = t.closest ? t.closest("[data-node]") : null;
        if (node) {
          hook.sel = node.getAttribute("data-node");
        } else if (edge) {
          hook.sel = edge.getAttribute("data-edge");
        } else if (t.closest && t.closest("#detail")) {
          return;                       // the panel handles its own clicks
        } else {
          hook.sel = null;
        }
        if (hook.sel) select(el, hook.sel); else clearSelection(el);
      };

      this.onDown = function (ev) {
        // A control is not a place on the canvas. Without this the press would
        // fall through to "empty background, so pan", and letting go of a pan
        // writes the camera — so every press of the sweep button would also have
        // been a store write.
        if (ev.target.closest && ev.target.closest("[data-sweep]")) return;
        // Hold ctrl (or cmd) and the canvas moves, whatever is under the cursor.
        // Panning by finding empty background is a hunt on a picture this dense —
        // and the denser the arrangement gets, the less background there is.
        if (ev.ctrlKey || ev.metaKey) {
          pan = {from: {x: ev.clientX, y: ev.clientY},
                 origin: {x: hook.cam.x, y: hook.cam.y}};
          el.classList.add("panning");
          ev.preventDefault();
          return;
        }
        const g = ev.target.closest("[data-node]");
        if (!g) {
          // Not a cell. A hive is the next thing worth grabbing: its frame and
          // its label are the group's handle, and moving a group is the gesture
          // that makes a 50-cell picture arrangeable at all — one drag instead of
          // twenty.
          const hg = ev.target.closest ? ev.target.closest("[data-hive]") : null;
          if (hg) {
            const id = hg.getAttribute("data-hive");
            const members = membersOf(el, id);
            const ids = members.map(m => m.getAttribute("data-node"));
            hive = {
              id: id,
              g: hg,
              // The block's ORIGIN as the server rendered it — never the
              // rectangle. The rectangle is derived from the cells inside, so it
              // moves when they do: anchoring a group to it meant a hive jumped on
              // the next render, and dragging one cell leftwards shoved its whole
              // hive to the right. What goes back on release is this point plus
              // the drag, and the server keeps the difference: what it stores is
              // the shift, because a point measured against a layout that every
              // arriving cell changes does not survive the colony growing.
              origin: {x: numAttr(hg, "data-ox"), y: numAttr(hg, "data-oy")},
              members: members.map(m => ({g: m, at: boxOf(m)})),
              from: userPoint(el, ev),
              delta: {x: 0, y: 0},
              // Every edge with an end inside this hive moves with it — and
              // every edge that ends on THIS hive's frame or on one above it,
              // because those frames move too. `edgesFor` walks each member's
              // ancestors, which covers the hive itself and everything over it.
              edges: edgesFor(el, ids.concat([id])),
            };
            hg.classList && hg.classList.add("dragging");
            ev.preventDefault();
            return;
          }
          // Empty canvas: pan. A picture larger than its frame with no way to
          // move is the same defect as no picture at all.
          pan = {from: {x: ev.clientX, y: ev.clientY},
                 origin: {x: hook.cam.x, y: hook.cam.y}};
          el.classList.add("panning");
          return;
        }
        const id = g.getAttribute("data-node");
        // Collect the attached edges ONCE, not per frame.
        // Not only this cell's own edges: every hive ABOVE it changes shape while
        // it moves, and an edge that ends on such a frame has to move with it.
        const attached = edgesFor(el, [id]);
        drag = {id: id, g: g, origin: boxOf(g), from: userPoint(el, ev),
                at: boxOf(g), edges: attached};
        g.setPointerCapture && g.setPointerCapture(ev.pointerId);
        ev.preventDefault();
      };

      this.onMove = function (ev) {
        if (hive) {
          const now = userPoint(el, ev);
          hive.delta = {x: now.x - hive.from.x, y: now.y - hive.from.y};
          if (frame) return;
          frame = requestAnimationFrame(function () {
            frame = null;
            if (!hive) return;
            const dx = Math.round(hive.delta.x), dy = Math.round(hive.delta.y);
            hive.members.forEach(function (m) {
              m.g.setAttribute("transform",
                "translate(" + (m.at.x + dx) + "," + (m.at.y + dy) + ")");
            });
            // Every frame follows from the cells, so moving the cells IS moving
            // the group: the hive keeps its shape, and every hive above it grows
            // or shrinks to hold it while the cursor is still down.
            applyFrames(el, hook.geom);
            const boxes = boxMap(el, false);
            hive.edges.forEach(function (p) {
              const a = boxes[p.getAttribute("data-from")];
              const b = boxes[p.getAttribute("data-to")];
              if (a && b) rubberBand(p, a, b);
            });
          });
          return;
        }
        if (pan) {
          hook.cam.x = pan.origin.x + (ev.clientX - pan.from.x);
          hook.cam.y = pan.origin.y + (ev.clientY - pan.from.y);
          applyCamera(el, hook.cam);
          return;
        }
        if (!drag) return;
        const now = userPoint(el, ev);
        drag.at = {x: drag.origin.x + (now.x - drag.from.x),
                   y: drag.origin.y + (now.y - drag.from.y)};
        if (frame) return;                       // coalesce to one frame
        frame = requestAnimationFrame(function () {
          frame = null;
          if (!drag) return;
          drag.g.setAttribute("transform",
            "translate(" + Math.round(drag.at.x) + "," + Math.round(drag.at.y) + ")");
          // A cell that leaves its hive has to be seen leaving it: every frame
          // above it grows or shrinks now, not on release.
          applyFrames(el, hook.geom);
          const boxes = boxMap(el, false);
          drag.edges.forEach(function (p) {
            const a = boxes[p.getAttribute("data-from")];
            const b = boxes[p.getAttribute("data-to")];
            if (a && b) rubberBand(p, a, b);
          });
        });
      };

      this.onUp = function (ev) {
        if (hive) {
          const done = hive;
          hive = null;
          done.g.classList && done.g.classList.remove("dragging");
          // The BOX origin, which is what the server stores for a group — one row
          // per hive, whatever its size. The members' own positions are not sent:
          // the server applies the group's offset to them and keeps any position
          // somebody gave a single cell by hand.
          hook.pushEvent("hive:moved", {
            id: done.id,
            x: Math.round(done.origin.x + done.delta.x),
            y: Math.round(done.origin.y + done.delta.y),
          });
          hook.dragged = true;          // the click that follows is the drag's tail
          return;
        }
        if (pan) {
          pan = null;
          el.classList.remove("panning");
          hook.saveCamera();
          return;
        }
        if (!drag) return;
        const done = drag;
        drag = null;
        done.g.releasePointerCapture && done.g.releasePointerCapture(ev.pointerId);
        // ONE event for the whole drag. The provisional DOM stays as it is —
        // the diff replaces it, and the server decides where the box ended up.
        hook.pushEvent("node:moved", {
          id: done.id, x: Math.round(done.at.x), y: Math.round(done.at.y)
        });
        hook.dragged = true;            // so the click that follows does not select
      };

      // Escape lets go of a selection without having to find empty canvas to
      // click on — same reflex as every other editor.
      this.onKey = function (ev) {
        if (ev.key !== "Escape" && ev.key !== "Esc") return;
        hook.sel = null;
        clearSelection(el);
      };

      el.addEventListener("pointerdown", this.onDown);
      el.addEventListener("pointermove", this.onMove);
      el.addEventListener("pointerup", this.onUp);
      el.addEventListener("pointercancel", this.onUp);
      el.addEventListener("wheel", this.onWheel, {passive: false});
      el.addEventListener("click", this.onClick);
      const doc = el.ownerDocument || document;
      if (doc && doc.addEventListener) doc.addEventListener("keydown", this.onKey);
    },

    unwire() {
      this.el.removeEventListener("pointerdown", this.onDown);
      this.el.removeEventListener("pointermove", this.onMove);
      this.el.removeEventListener("pointerup", this.onUp);
      this.el.removeEventListener("pointercancel", this.onUp);
      this.el.removeEventListener("wheel", this.onWheel);
      this.el.removeEventListener("click", this.onClick);
      const doc = this.el.ownerDocument || document;
      if (doc && doc.removeEventListener) doc.removeEventListener("keydown", this.onKey);
    },
  };

  // The one name the binary and this file agree on: the dead render offers the
  // slot, the surface fills it.
  root.SurfaceHooks = Object.assign(root.SurfaceHooks || {}, {Canvy: Canvy});
})(typeof window !== "undefined" ? window : globalThis);
