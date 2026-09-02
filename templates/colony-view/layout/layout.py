"""The layout: a topology snapshot in, one view out.

THIS FILE IS THE SOURCE. `config.json` carries a byte-identical copy of it in
`script_inline`, and a drift lock compares the two. Edit here.

Why the copy exists at all: a `code` cell's `script_path` is handed to the
interpreter verbatim with no working directory of its own (the factory ignores
`cell_dir` -- `code` is stateless), so a relative path would resolve against the
daemon's cwd, and an absolute path baked into a template is the exported-tree
defect class of [#20](https://github.com/mmeyerlein/meclaw/issues/20). So the
runtime form is `script_inline`, and this file is what a person reads, greps,
diffs and runs:

    python3 layout.py < a-snapshot.json

The browser half is carried the same way: `colony-view.js` and `colony-view.css`
are files a person diffs, and their content sits verbatim in the two constants
below. The marker lines around each constant are the extraction contract -- one
raw triple-quoted literal per constant, no triple quote inside it, each marker
line exactly once in this file -- so a lock can pull the three copies apart and
compare them without parsing Python.

# What this cell is

It is the whole picture, and it draws it as a COMPONENT TREE rather than as
HTML. A `display` holds the tree, the component library and the page; this cell
says what belongs in it. A new kind of thing on the canvas is therefore a
template edit -- one more component and one more branch -- and never a release.

It is NOT the owner of the topology. That comes from the colony's read-only
graph endpoint, by message, through this hive's own probe lane, on a timer --
never read out of a database, and never fetched inside a browser's request (a
reply from that endpoint arrives on a fresh envelope with no context, so it
could not be correlated back to whoever asked).

It IS the owner of where a cell sits, and since 1.0.1 it says so on every tick:
each node's `x`/`y` is the flow's, rewritten every time, so the flow's own
guarantees -- disjoint hives, nested frames, a frame 1.08x-1.43x the boxes it
holds -- are what a viewer actually sees.

What it is NOT the owner of is what a HAND did. That travels beside the flow's
position as `hand` (`"dx,dy"`, one prop on purpose) plus a `pinned` marker, and
those two the tree entry
declares `keep`: on an update to an object the display already holds they are
left out, and since an update merges per key the browser's value stands. So an
arrangement survives every tick without this cell hearing about it, and there is
nothing to read back.

Until 1.0.1 the kept props were `x` and `y` themselves, and that is
[#544](https://github.com/mmeyerlein/meclaw/issues/544): a position frozen at
the tick its object was created, on a colony that grows a cell an hour, is a
collage of a dozen incompatible layouts. Measured on a running colony: 1 of 104
boxes stood where the current flow put it, 208 of 215 unrelated hive pairs
overlapped and one frame ran 299x the area of the three cells inside it. Nobody
had dragged anything -- nobody could, because `data-oid` named an object one
tree level away from the one the display had minted. The pin was the mere
presence of a coordinate, which `canvy@2.1.8` had already replaced with a marker
of its own; the re-cut of #455 lost that, and this brings it back. The marker is
the whole of it: a hand's offset is NOT bounded, because a bound was tried and
withdrawn (`hive_of_cell_frames` carries the measurement). What keeps the picture
honest is that nothing moves that a hand did not move, every box that moved says
so, and the detail panel hands it back to the layout.

# One pass

A snapshot arrives with `body.graph` -- compute the picture and emit exactly one
message on the `view` lane. Anything else: emit nothing.

That is the whole cell, and the count is the point. `canvy` needed three passes
because it wrote into a display it had to interrogate first, and its third pass
existed only to recognise the acknowledgement of its own write: without that
discriminator one tick became two, two became four, and the routing loop wedged
on a full mailbox inside twenty seconds
([#161](https://github.com/mmeyerlein/meclaw/issues/161)). A view is a statement
rather than a patch, so nothing here answers this cell and no such loop can
form.
"""
import hashlib
import json
import sys

# The browser half. Each constant is a verbatim copy of the file named in the
# markers around it -- one raw triple-quoted literal, nothing escaped -- so a
# lock can cut it out of this file, out of `config.json` and out of the file
# itself and compare all three. The layout writes them into the shell's two raw
# props: they are the largest thing this cell ever says, and they are the same
# on every tick.
# --- BEGIN colony-view.js ---
CLIENT_JS = r"""// colony-view's browser half: edge routing, drag, camera.
//
// THIS FILE IS THE SOURCE. `layout/layout.py` carries a byte-identical copy in
// its `CLIENT_JS` constant, and `layout/config.json` carries that file again in
// `script_inline`; a drift lock compares all three copies. The layout cell
// writes it into the `client_js` prop of the shell component, which renders it
// raw inside a `<script>` tag. A `display` serves its page out of a materialised
// tree, and the body of that tree is parsed by the browser BEFORE the shell's
// own `<script src="/@client/...">` tags -- so by the time the LiveView boot
// reads `window.SurfaceHooks`, this file has already put `ColonyView` there.
//
// # Part one: the geometry (`TopoGeom`)
//
// Carried over from `canvy` unchanged, and deliberately so -- it is the part
// that was visibly wrong twice. What is NOT carried over is the node test that
// proved it: see the README. The client path is still never proven over the
// websocket alone, and that is now a stated gap rather than a covered one.
//
// The first version drew a straight line between two box centres and trimmed the
// ends with `Math.min` over the two axis ratios. That is not the boundary of a
// rectangle, so arrowheads landed inside the box and lines crossed straight
// through cells that happened to sit between the endpoints. So: leave a box
// through the side that faces the target, orthogonally, and approach the target
// the same way. Same idea as draw.io's orthogonal router, minus the obstacle
// avoidance -- with a stub on each end, a line no longer starts under its own
// source, which is what actually made it unreadable.
//
// # Part two: the hook
//
// A drag is an `object:set` on the dragged object's own `x`/`y` -- the display's
// local lane, which never enters the colony router. A hive drag is the same
// gesture repeated over the members it moves: there is no group row, because
// there is no store behind this picture, only objects. And the camera is purely
// local state: nothing is sent, nothing is stored, and a reload starts from the
// picture's own frame again.
//
// There is no pin marker here and no "release to the layout" button. A move
// survives because the node's tree entry declares `keep: ["x","y"]` and the
// display leaves those two props out of an update, so the browser's value
// stands. Nothing is ever read back into this view.

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
    // (the boundary rule — overview § The hive boundary) has to leave and land on
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
  /// § The hive boundary) runs between a hive and a cell INSIDE it: two real
  /// boxes, but not two boxes side by side. "Which side of `a` faces `b`" has no
  /// answer when every side of the frame faces the cell, and `side()` answered it
  /// anyway — with the OUTWARD normal. So the line left the frame through its
  /// outer wall, ran around the outside and came back in. All nine door edges in
  /// the live colony were drawn that way.
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
  ///
  /// **Concatenation and not template literals, and that is not a style
  /// choice.** This file is copied into the layout cell's `script_inline`, and
  /// every string value of a shipped `config.json` goes through the substrate's
  /// environment substitution on every read (spec § Variable substitution). A
  /// dollar-brace form in there is not a template literal to the colony, it is
  /// an env token with no value — `env_var_missing`, at boot, for the whole
  /// cell. The doubled-dollar escape exists, but a client that is only correct
  /// after an escaping pass is a client whose file and whose shipped copy say
  /// different things. So this file carries no dollar-brace at all, and a test
  /// keeps it that way.
  function rounded(pts, r) {
    r = r === undefined ? 6 : r;
    const out = ["M" + fmt(pts[0].x) + "," + fmt(pts[0].y)];
    for (let i = 1; i < pts.length - 1; i++) {
      const p = pts[i], prev = pts[i - 1], next = pts[i + 1];
      const inLen = Math.hypot(p.x - prev.x, p.y - prev.y);
      const outLen = Math.hypot(next.x - p.x, next.y - p.y);
      if (inLen < 0.5 || outLen < 0.5) continue;           // degenerate corner
      const rr = Math.min(r, inLen / 2, outLen / 2);
      const a = {x: p.x - (p.x - prev.x) / inLen * rr, y: p.y - (p.y - prev.y) / inLen * rr};
      const b = {x: p.x + (next.x - p.x) / outLen * rr, y: p.y + (next.y - p.y) / outLen * rr};
      out.push("L" + fmt(a.x) + "," + fmt(a.y));
      if (rr > 0.5) {
        out.push("Q" + fmt(p.x) + "," + fmt(p.y) + " " + fmt(b.x) + "," + fmt(b.y));
      }
    }
    const last = pts[pts.length - 1];
    out.push("L" + fmt(last.x) + "," + fmt(last.y));
    return out.join(" ");
  }

  function fmt(n) {
    return Math.round(n * 10) / 10;
  }

  /// Does a segment pass through a box? Exported so a harness can prove the
  /// routing keeps clear of its own endpoints' boxes.
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
  /// result was `MNaN,NaN`, i.e. an invisible line. Every property test of the
  /// router passed, because they all used `route(...).d` — the defect lived in
  /// the ONE expression no test evaluated. Now there is only one way to spell it.
  function edgePath(a, b, w, h, lane, lanes) {
    return route(a, b, w, h, lane, lanes).d;
  }

  const api = {STUB, LANE, side, anchor, route, rounded, edgePath, segmentHitsBox,
               freeLanes, corridor, crossings, contains, innerSide, faceAt, ends};
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  root.TopoGeom = api;
})(typeof window !== "undefined" ? window : globalThis);

// ---------------------------------------------------------------------------
// The LiveView hook. Three jobs, all of them presentational.
//
// # 1. Draw the edges
//
// The layout cell writes each edge as its ENDPOINTS and a lane number, never as
// a finished path: the orthogonal routing above is presentation, and computing a
// `d` on both sides would be one algorithm in two languages. So every time a
// diff lands, this walks the edges and fills in their `d`.
//
// # 2. Own the drag, and write it as an object patch
//
// Between pointerdown and pointerup this moves a box and the lines attached to
// it. On release it sends `object:set` for the two props the node component
// declared `editable` — `x` and `y`. That event is the display's LOCAL lane:
// it is written to the display's own database and diffed to every viewer without
// a single message entering the colony router. A drag is therefore not a
// conversation with anybody.
//
// A hive drag is the same gesture, repeated over its members. There is no group
// row and nothing to measure a group against: a point measured against a layout
// that every arriving cell changes does not survive the colony growing, which
// was measured once and cost twelve of nineteen hand-placed frames. Here the
// members' own `x`/`y` ARE the record, so moving the group is moving the members
// and the frames follow, exactly as they do during the drag.
//
// # 3. Own the camera, and tell nobody
//
// Pan and zoom are local state and stay local: where a person is looking is a
// fact about that person and not about the colony, so writing it back would
// spend a round trip on a gesture that means nothing to anybody else. The
// picture's own `viewBox` frames the whole drawing before any script runs, so a
// fresh load is legible without a stored camera.
//
// No `phx-update="ignore"` anywhere: the server renders everything and the diff
// must win. The provisional DOM is overwritten on purpose — that is the whole
// SSR statement.
(function (root) {
  "use strict";
  if (typeof document === "undefined") return;   // required by the test, not run

  const G = root.TopoGeom;
  const NODE_W = 150, NODE_H = 38;

  /// Every translate on a group, in order. A node carries TWO since 1.0.1: the
  /// flow's spot, which the layout cell rewrites every tick, and the hand's
  /// offset beside it (`hand`, one prop, `"dx,dy"`), which only a drag writes.
  function translatesOf(g) {
    const out = [];
    const re = /translate\((-?[\d.]+)[,\s]\s*(-?[\d.]+)\)/g;
    const t = g.getAttribute("transform") || "";
    let m;
    while ((m = re.exec(t))) out.push({x: +m[1], y: +m[2]});
    return out;
  }

  /// Where the FLOW put a box: the first translate, and the constraint a hand's
  /// offset is measured against.
  function flowOf(g) {
    const t = translatesOf(g);
    return t.length ? t[0] : {x: 0, y: 0};
  }

  /// What a hand added to it: the second translate, zero where there is none.
  function offsetOf(g) {
    const t = translatesOf(g);
    return t.length > 1 ? t[1] : {x: 0, y: 0};
  }

  /// Where a box actually IS: the flow's spot plus the hand's offset. Every
  /// piece of geometry in this file goes through here, which is why a drag can
  /// replace the whole transform with one provisional translate and nothing
  /// downstream notices.
  function boxOf(g) {
    const t = translatesOf(g);
    let x = 0, y = 0;
    t.forEach(function (p) { x += p.x; y += p.y; });
    return {x: x, y: y};
  }

  /// The rectangle the browser most recently derived for a hive, so a write
  /// back to the store only happens when it actually says something new.
  const lastFrame = {};

  /// What this browser has written and the screen has not confirmed yet.
  ///
  /// A prop write is answered by a diff, and a diff re-renders the box from the
  /// props the display holds AT THAT MOMENT -- which, for the first diff after a
  /// drag, can still be the value from before it. The box then jumps back to
  /// where it started, the frames are derived from that, and the picture
  /// corrects itself a beat later. Measured under GH #544 as a frame drawn at
  /// its old size once before settling. So what this browser wrote stands here
  /// until a diff actually says it, and every re-render puts it back.
  const pending = {};

  function centre(box) {
    return {x: box.x + NODE_W / 2, y: box.y + NODE_H / 2};
  }

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
  /// stays where the frame used to be, which is the "no edges while moving"
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

  /// A v-lane says out loud which lane made it legal (GH #559), and the picture
  /// says it twice: dashed, so a declared deep edge is not read as one more
  /// ordinary hop, and a `<title>` on the group, so pointing at it names the
  /// lane. Both are drawn here and nowhere else -- `colony-view-edge` stays
  /// `editable: []`, nothing is written back, and an edge that declares no lane
  /// is left exactly as it was drawn before this existed.
  ///
  /// The dash is an inline style and not a class, deliberately: half a real
  /// colony's edges carry a condition, the stylesheet already dashes those, and
  /// a class would lose the argument to `.cond` on exactly the edges where the
  /// lane matters most.
  function markLane(p) {
    const lane = p.getAttribute("data-vlane") || "";
    const g = p.parentNode;
    if (!g || !g.querySelector) return;
    let t = g.querySelector("title");
    if (!lane) {
      p.style.strokeDasharray = "";
      if (t) g.removeChild(t);
      return;
    }
    p.style.strokeDasharray = "2 4";
    if (!t) {
      t = document.createElementNS("http://www.w3.org/2000/svg", "title");
      g.insertBefore(t, g.firstChild);
    }
    t.textContent = "lane: " + lane;
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
      markLane(p);
      // The fat invisible twin follows the same path — it is what a mouse hits.
      const hit = p.parentNode && p.parentNode.querySelector
        ? p.parentNode.querySelector("path.edge-hit") : null;
      if (hit) hit.setAttribute("d", d);
    });
  }

  /// Everything BELOW a hive: every cell at any depth, not just its direct
  /// children. A hive's frame is the frame around its whole subtree, so a drag
  /// that moved only the direct children would leave the nested hives behind for
  /// one round trip and then snap them.
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

  /// The layout constants, read from the markup the layout cell wrote.
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
  /// The same union the layout cell computes: a hive's own cells padded, plus
  /// every child's frame grown by the nesting inset. Which is why dragging a cell
  /// out of a hive grows that hive AND every hive above it while the cursor is
  /// still down — the frames are derived, so they can be derived again at 60 Hz
  /// instead of waiting for a round trip. A frame that only updates on release is
  /// a frame that lies for as long as you are looking at it.
  function frameMap(el, geom) {
    const own = {}, kids = {}, seen = {};
    // A box nobody can see does not shape a frame. With the toggle off -- the
    // default -- the unwired leftovers of every past rewiring are hidden, and a
    // frame drawn around them would be a frame around nothing anybody is
    // looking at.
    const hiding = el.classList && el.classList.contains("hide-unwired");
    el.querySelectorAll("[data-node]").forEach(function (g) {
      if (hiding && g.classList && g.classList.contains("unwired")) return;
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

  /// Put a box back into the two-translate form the layout writes.
  ///
  /// A drag replaces the whole transform with ONE provisional translate, and the
  /// server diff normally puts the pair back. When a drag writes nothing --
  /// a press that moved nothing, a gesture that ended where it started -- no
  /// diff comes, and the box was left with a single translate. From then on
  /// `flowOf` read the absolute position as the flow's and `offsetOf` read
  /// zero, so the next drag measured against a base that does not exist and the
  /// box would not move at all. Measured in a browser under GH #544: one
  /// blocked drag was enough to make a box permanently unmovable.
  function restore(g, flow, off) {
    g.setAttribute("transform",
      "translate(" + Math.round(flow.x) + "," + Math.round(flow.y) + ") " +
      "translate(" + Math.round(off.x) + "," + Math.round(off.y) + ")");
  }

  /// Put every unconfirmed write back after a morph, and forget the confirmed.
  function reconcile(el) {
    const ids = Object.keys(pending);
    if (!ids.length) return;
    el.querySelectorAll("[data-node]").forEach(function (g) {
      const oid = g.getAttribute("data-oid");
      const want = pending[oid];
      if (!want) return;
      const have = offsetOf(g);
      if (Math.round(have.x) === want.x && Math.round(have.y) === want.y) {
        delete pending[oid];             // the screen agrees; nothing to hold
        return;
      }
      restore(g, flowOf(g), want);
    });
  }

  /// Redraw every hive rectangle from the current cell positions.
  ///
  /// With `hook`, the derived rectangle is also written BACK to the hive's
  /// object. The layout cell cannot do this: a hand's offset lives in the
  /// display and never reaches it, so the rectangle it computes is the flow's.
  /// Before GH #544 that third geometry was what a screenshot or an export got
  /// -- 96 of 104 cells lay outside the frame the store held, while the browser
  /// looked correct because it re-derived. The browser is the only half that
  /// knows, so the browser says it.
  function applyFrames(el, geom, hook) {
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
      if (!hook) return;
      const id = g.getAttribute("data-hive");
      const now = [Math.round(r.x), Math.round(r.y),
                   Math.round(r.w), Math.round(r.h)];
      const was = lastFrame[id];
      if (was && was[0] === now[0] && was[1] === now[1] &&
          was[2] === now[2] && was[3] === now[3]) return;
      lastFrame[id] = now;
      const oid = g.getAttribute("data-oid");
      if (!oid) return;
      ["x", "y", "w", "h"].forEach(function (k, i) {
        hook.setProp(oid, k, now[i]);
      });
    });
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
  /// User units per CSS pixel of the fitted SVG, camera excluded.
  ///
  /// The camera's translate lives INSIDE the viewBox mapping, so a pan that
  /// wants the picture to follow the pointer 1:1 must feed the camera
  /// pixel-deltas divided by exactly this factor. Feeding raw pixels made the
  /// picture crawl at the viewBox-fit fraction of the pointer speed.
  function fitScale(el) {
    const svg = el.querySelector("svg.stage");
    const m = svg && svg.getScreenCTM && svg.getScreenCTM();
    return m && m.a ? m.a : 1;
  }

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
  // Entirely client-side and deliberately so: what is selected is not a fact
  // about the colony, and a round trip per click would make reading a graph cost
  // cell calls. Everything it needs is already in the markup.

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
    const d = el.querySelector(".detail");
    if (d) d.innerHTML = '<p class="empty">Click a cell or an edge.</p>';
  }

  /// Fill the panel and dim what is not involved. `id` is a cell path or an edge
  /// id; anything else clears.
  function select(el, id, hook) {
    const detail = el.querySelector(".detail");
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
    // The pin, and the way back out of it. A box a hand moved carries the
    // `pinned` marker and an offset against the flow's spot; a box nobody has
    // touched carries neither and simply follows the layout. Releasing is
    // therefore two writes — the marker and the offset, one prop each — and the
    // next tick puts the box back where the flow wants it.
    //
    // The button exists because the alternative is a picture a person can
    // arrange and never un-arrange: `canvy@2.1.8` learnt that, and the re-cut
    // of #455 shipped without it (GH #544).
    const pinned = !!cellG.getAttribute("data-pinned");
    // How far this box is from where the layout wanted it -- which is exactly
    // what releasing it undoes. Saying "outside its hive" would never fire:
    // since 1.0.1 the frame FOLLOWS its cells, so a box is inside its own hive
    // by construction however far it was dragged. The offset is the honest
    // number, and it is the one a person can act on.
    const off = offsetOf(cellG);
    detail.innerHTML =
      '<dl class="kv"><dt>cell</dt><dd>' + esc(id) + "</dd></dl>" +
      '<dl class="kv"><dt>type</dt><dd>' + esc(ty ? ty.textContent : "") + "</dd></dl>" +
      '<dl class="kv"><dt>placed</dt><dd>' + (pinned
        ? '<button class="release" type="button">by hand — release</button>' +
          '<div class="warn">' + Math.round(off.x) + ", " + Math.round(off.y) +
          ' px from where the layout put it — release puts it back, and the' +
          ' frame follows</div>'
        : '<span class="chip">by the layout</span>') + "</dd></dl>" +
      '<dl class="kv"><dt>in (' + ins.length + ")</dt><dd>" + row(ins, e => e.from) + "</dd></dl>" +
      '<dl class="kv"><dt>out (' + outs.length + ")</dt><dd>" + row(outs, e => e.to) + "</dd></dl>";
    detail.querySelectorAll(".rel").forEach(function (n) {
      n.addEventListener("click", function () { select(el, n.getAttribute("data-rel"), hook); });
    });
    const release = detail.querySelector(".release");
    if (release && hook && hook.setProp) {
      release.addEventListener("click", function () {
        const oid = cellG.getAttribute("data-oid");
        cellG.removeAttribute("data-pinned");
        pending[oid] = {x: 0, y: 0};
        hook.setProp(oid, "pinned", "");
        hook.setProp(oid, "hand", "0,0");
      });
    }
  }

  const ColonyView = {
    mounted() {
      // The camera is local state and starts at identity. It is deliberately NOT
      // read out of the markup: there is nothing in the picture that remembers a
      // camera any more, because nothing writes one.
      this.cam = {x: 0, y: 0, z: 1000};
      this.geom = geometryOf(this.el);
      // The frame is part of the camera. The server recomputes the `viewBox`
      // from the content's bounding box on every layout tick, and after a drag
      // that box HAS changed — so up to a minute later a diff arrived carrying
      // a new frame, and the whole picture jumped to re-fit it. To the person
      // looking at it that is "the page reloaded and re-centred itself". So
      // the viewBox the page mounted with is recorded here and pinned across
      // every later morph: a fresh load still adopts the server's fit (this
      // very line reads it), but a session's view moves only by its own hand.
      const svg = this.el.querySelector("svg.stage");
      this.vb = svg && svg.getAttribute ? svg.getAttribute("viewBox") : null;
      // Which browser half this picture was written by, at the moment this tab
      // started running. A `<script>` inside a LiveView morph is not executed,
      // so this hook keeps running for the life of the tab while the server may
      // move on. Comparing the two is the only way a stale tab can know.
      this.clientId = this.el.getAttribute("data-client") || "";
      this.hiding = true;              // the server's own default, mirrored
      this.wire();
      applyCamera(this.el, this.cam);
      applyFrames(this.el, this.geom, this);
      drawEdges(this.el);
    },
    // A diff re-renders the slot it changed, so the edges of the boxes it moved
    // have to be drawn again, and the camera transform re-applied to whatever
    // viewport element is now in the DOM. Re-applying is not a workaround for the
    // SSR model, it IS the model: the client owns the view, the server owns the
    // picture.
    //
    // The FRAMES are re-derived too, and this line is load-bearing: LiveView
    // morphs the whole container back to the server tree on every diff, and the
    // server's hive rectangles only catch up on the next layout tick. Without
    // re-deriving here, dropping a cell snapped every frame back to the stale
    // server geometry for up to a minute -- seen as "the hive jumps back, then
    // corrects itself seconds later". Frames are derived from the cells, so the
    // client can always recompute them from what the morph just delivered.
    updated() {
      this.geom = geometryOf(this.el);
      // The morph just wrote the server's freshly-fitted viewBox; put the
      // session's own frame back before anything is painted (this runs in the
      // same task as the patch). Without it the picture re-centres on the
      // first tick after any drag — see mounted() for the full story.
      const svg = this.el.querySelector("svg.stage");
      if (svg && this.vb) svg.setAttribute("viewBox", this.vb);
      // Anything this browser wrote and the screen has not confirmed goes back
      // on BEFORE the frames are derived -- a frame derived from a box the diff
      // just put back where it started is a frame drawn at the wrong size.
      reconcile(this.el);
      // The morph writes the server's container class back, and whether you are
      // looking at the leftovers is a fact about this browser, not about the
      // colony. So it is restored here, beside the camera and the viewBox.
      this.el.classList.toggle("hide-unwired", this.hiding);
      applyCamera(this.el, this.cam);
      applyFrames(this.el, this.geom, this);
      drawEdges(this.el);
      // A tab that has been open across a template change runs the client it
      // loaded with, and every gesture it makes speaks a vocabulary the screen
      // may no longer accept -- silently, because a refused prop write is a
      // receipt to the app and nothing to the browser. So the tab says it.
      const now = this.el.getAttribute("data-client") || "";
      if (this.clientId && now && now !== this.clientId) {
        this.el.classList.add("stale");
      }
      if (this.sel) select(this.el, this.sel, this); else clearSelection(this.el);
    },
    destroyed() { this.unwire(); },

    /// One `object:set`. The display writes it locally and diffs it to every
    /// viewer; nothing enters the colony.
    setProp(oid, prop, value) {
      if (!oid) return;
      this.pushEvent("object:set", {id: oid, prop: prop, value: value});
    },

    wire() {
      const el = this.el, hook = this;
      let drag = null;
      let hive = null;
      let pan = null;
      let frame = null;

      // Zoom around the cursor: the point under the pointer stays under it, which
      // is the only zoom that does not feel like being teleported. Purely local —
      // nothing is pushed and nothing is stored.
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
      };

      // A click that did not drag is a selection. Decided on pointerUP by whether
      // the pointer moved, so a drag never selects and a click never has to be
      // held still — the same rule the reference viewer uses.
      this.onClick = function (ev) {
        if (hook.dragged) { hook.dragged = false; return; }
        const t = ev.target;
        const edge = t.closest ? t.closest("[data-edge]") : null;
        const node = t.closest ? t.closest("[data-node]") : null;
        if (node) {
          hook.sel = node.getAttribute("data-node");
        } else if (edge) {
          hook.sel = edge.getAttribute("data-edge");
        } else if (t.closest && t.closest(".detail")) {
          return;                       // the panel handles its own clicks
        } else {
          hook.sel = null;
        }
        if (hook.sel) select(el, hook.sel, hook); else clearSelection(el);
      };

      this.onDown = function (ev) {
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
          // A hive moves by its BODY — that is the gesture a hand reaches
          // for, and the label alone is a glyph-lottery on a zoomed-out
          // picture. The DEEPEST hive whose
          // rectangle holds the pointer is the one grabbed (decided
          // geometrically, so paint order and overlapping fills cannot steal
          // the grab), which is what makes the gesture safe where it once
          // relocated a whole colony: back then the OUTERMOST fill covered
          // everything and won every grab; now the grab is the box you are
          // visually inside of. The camera pans on true background — outside
          // every frame — and on ctrl-drag anywhere (handled above).
          //
          // An OUTERMOST frame is never grabbed, and that is GH #544's second
          // half. A frame with no parent frame in the picture is not a frame
          // around anything -- it is the canvas: on a real colony it holds 96 %
          // empty space, so almost every press that misses a box lands inside
          // it and inside nothing else. Measured on a live colony: one press on
          // that emptiness dragged all 108 cells and marked every one of them
          // hand-placed, which is the whole picture leaving the layout in a
          // single gesture. So a root frame pans, and the group drag is offered
          // for the frames that are actually groups.
          const roots = {};
          el.querySelectorAll("[data-hive]").forEach(function (g) {
            roots[g.getAttribute("data-hive") || ""] = true;
          });
          const isRoot = function (path) {
            const cut = path.lastIndexOf("/");
            return cut < 0 || !roots[path.slice(0, cut)];
          };
          let hg = ev.target.closest ? ev.target.closest("[data-hive]") : null;
          if (hg && isRoot(hg.getAttribute("data-hive") || "")) hg = null;
          if (ev.clientX !== undefined) {
            const OUT = 4;
            let best = null;
            el.querySelectorAll("[data-hive]").forEach(function (g) {
              const r = g.querySelector && g.querySelector("rect");
              if (!(r && r.getBoundingClientRect)) return;
              if (isRoot(g.getAttribute("data-hive") || "")) return;
              const b = r.getBoundingClientRect();
              if (ev.clientX >= b.left - OUT && ev.clientX <= b.right + OUT &&
                  ev.clientY >= b.top - OUT && ev.clientY <= b.bottom + OUT) {
                const depth = (g.getAttribute("data-hive") || "").split("/").length;
                if (!best || depth >= best.depth) best = {g: g, depth: depth};
              }
            });
            if (best) hg = best.g;
          }
          if (hg) {
            const id = hg.getAttribute("data-hive");
            const members = membersOf(el, id);
            const ids = members.map(m => m.getAttribute("data-node"));
            hive = {
              id: id,
              g: hg,
              members: members.map(m => ({g: m, at: boxOf(m),
                                          flow: flowOf(m), off: offsetOf(m),
                                          oid: m.getAttribute("data-oid")})),
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
        drag = {id: id, oid: g.getAttribute("data-oid"), g: g, origin: boxOf(g),
                flow: flowOf(g), off: offsetOf(g),
                pinned: !!g.getAttribute("data-pinned"),
                from: userPoint(el, ev), at: boxOf(g), edges: attached};
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
          const k = fitScale(el);
          hook.cam.x = pan.origin.x + (ev.clientX - pan.from.x) / k;
          hook.cam.y = pan.origin.y + (ev.clientY - pan.from.y) / k;
          applyCamera(el, hook.cam);
          return;
        }
        if (!drag) return;
        const now = userPoint(el, ev);
        drag.offAt = {x: drag.off.x + (now.x - drag.from.x),
                      y: drag.off.y + (now.y - drag.from.y)};
        drag.at = {x: drag.flow.x + drag.offAt.x, y: drag.flow.y + drag.offAt.y};
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
          const dx = Math.round(done.delta.x), dy = Math.round(done.delta.y);
          done.members.forEach(function (m) {
            restore(m.g, m.flow, {x: Math.round(m.off.x) + dx,
                                  y: Math.round(m.off.y) + dy});
          });
          if (dx || dy) {
            // One patch per member and per prop. There is no group row to
            // write: the members' own offsets ARE the record, so the group's
            // move is exactly the sum of its members' moves and nothing has to
            // be reconciled afterwards. The OFFSET is what is written -- where
            // the flow puts each member is the layout cell's word, and it says
            // it again on the next tick.
            done.members.forEach(function (m) {
              const nx = Math.round(m.off.x) + dx, ny = Math.round(m.off.y) + dy;
              pending[m.oid] = {x: nx, y: ny};
              hook.setProp(m.oid, "hand", nx + "," + ny);
              if (!m.g.getAttribute("data-pinned")) {
                m.g.setAttribute("data-pinned", "1");
                hook.setProp(m.oid, "pinned", "1");
              }
            });
            hook.dragged = true;        // the click that follows is the drag's tail
          }
          // A press that never moved is a CLICK, and with the body as the
          // grab surface every click starts here — swallowing it would kill
          // edge and cell selection inside every frame.
          return;
        }
        if (pan) {
          pan = null;
          el.classList.remove("panning");
          return;
        }
        if (!drag) return;
        const done = drag;
        drag = null;
        done.g.releasePointerCapture && done.g.releasePointerCapture(ev.pointerId);
        // Two events for the move and a third the first time a box is moved at
        // all. The provisional DOM stays as it is — the diff replaces it, and
        // the cell decides what the object now says. A press that never moved
        // is a CLICK: it writes nothing (two no-op writes per selection click,
        // before this guard) and it must not swallow the selection that
        // follows.
        const off = done.offAt || done.off;
        const nx = Math.round(off.x), ny = Math.round(off.y);
        // Whatever happens next, the box goes back into the two-translate form.
        // A gesture that writes nothing gets no diff, and a box left with one
        // translate has lost the line between where the flow put it and where a
        // hand did -- after which it cannot be moved again at all.
        restore(done.g, done.flow, {x: nx, y: ny});
        if (nx !== Math.round(done.off.x) || ny !== Math.round(done.off.y)) {
          // ONE prop, and that is the whole of why a drag no longer flickers.
          // A browser writes a prop at a time and the display diffs a prop at a
          // time; an offset spelled as two props reached the page as two
          // pictures, and the frames -- derived from where the boxes are -- were
          // derived once from the half-moved one. Measured: three rectangles
          // painted for one drag, the middle 971 wide and still 92 high.
          pending[done.oid] = {x: nx, y: ny};
          hook.setProp(done.oid, "hand", nx + "," + ny);
          if (!done.pinned) {
            done.g.setAttribute("data-pinned", "1");
            hook.setProp(done.oid, "pinned", "1");
          }
          hook.dragged = true;          // so the click that follows does not select
        }
      };

      // Escape lets go of a selection without having to find empty canvas to
      // click on — same reflex as every other editor.
      this.onKey = function (ev) {
        if (ev.key !== "Escape" && ev.key !== "Esc") return;
        hook.sel = null;
        clearSelection(el);
      };

      // The toggle. It is a CLASS on the container and nothing else -- no
      // server round trip, no stored preference: which cells you are looking at
      // is the same kind of local fact as where you are looking, and both stay
      // in this browser.
      this.onToggle = function (ev) {
        const b = ev.target.closest ? ev.target.closest(".unwired-toggle") : null;
        if (!b) return;
        ev.preventDefault();
        ev.stopPropagation();
        hook.hiding = !hook.hiding;
        el.classList.toggle("hide-unwired", hook.hiding);
        applyFrames(el, hook.geom, hook);
        drawEdges(el);
      };
      el.addEventListener("click", this.onToggle, true);
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
      this.el.removeEventListener("click", this.onToggle, true);
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

  // The one name the binary and this file agree on: the cell's shell offers the
  // slot, the page fills it.
  root.SurfaceHooks = Object.assign(root.SurfaceHooks || {},
                                   {ColonyView: ColonyView});
})(typeof window !== "undefined" ? window : globalThis);
"""
# --- END colony-view.js ---

# --- BEGIN colony-view.css ---
CLIENT_CSS = r"""/* colony-view's own look.

   THIS FILE IS THE SOURCE. `layout/layout.py` carries a byte-identical copy in
   its `CLIENT_CSS` constant and `layout/config.json` carries that file again in
   `script_inline`; a drift lock compares all three. The layout cell writes it
   into the `client_css` prop of the shell component, which renders it raw inside
   a `<style>` tag.

   It belongs to the TEMPLATE, not to the binary: a view that should look
   different costs a template edit, not a release. */

.colony-view{
  --bg:#fbfbfa; --ink:#1a1a18; --muted:#6b6b66; --line:#dcdcd8;
  --panel:#ffffff; --accent:#2f6f4f; --edge:#9a9a94; --edge-hot:#c2410c;
  --sel:#2f6f4f; --bad:#b3261e;
  /* One tint per DEPTH, so nesting is readable without counting dashes or
     measuring rectangles. The tints are faint and they STACK: hives are emitted
     parent-first, so a child paints over its parent and each step adds a shade.
     Ten depths, because a real colony reaches eight
     (`/org/<org>/member/<who>/assistants/<name>/talky/keeper`); deeper keeps the
     last tint, which is still a tint — a hive with no fill would read as "not a
     hive". Kept in sync with HIVE_DEPTH_TINTS in layout.py, and a test reads
     both. */
  --hive1:rgba(47,111,79,.055); --hive2:rgba(40,120,170,.060);
  --hive3:rgba(120,90,180,.060); --hive4:rgba(196,120,40,.065);
  --hive5:rgba(176,60,110,.055); --hive6:rgba(30,140,140,.060);
  --hive7:rgba(150,140,40,.065); --hive8:rgba(90,90,200,.055);
  --hive9:rgba(200,90,60,.060); --hive10:rgba(120,120,120,.075);
}
/* The viewer's theme has three states: an explicit choice, and the default
   "system" setting, which stamps nothing and is separated only by the media
   query. Both are answered, and the light palette above is the one that stands
   when neither block applies. */
@media (prefers-color-scheme: dark){ :root:not([data-theme=light]) .colony-view{
  --bg:#17181b; --ink:#e9e9e6; --muted:#9a9a94; --line:#34353b;
  --panel:#1e1f23; --accent:#7fbf9a; --edge:#61626a; --edge-hot:#f97316;
  --sel:#7fbf9a; --bad:#f2b8b5;
  --hive1:rgba(127,191,154,.070); --hive2:rgba(90,170,225,.075);
  --hive3:rgba(170,140,235,.075); --hive4:rgba(240,160,70,.080);
  --hive5:rgba(235,120,170,.070); --hive6:rgba(80,205,205,.075);
  --hive7:rgba(215,205,90,.080); --hive8:rgba(140,140,245,.070);
  --hive9:rgba(245,130,95,.075); --hive10:rgba(190,190,190,.090);
}}
:root[data-theme=dark] .colony-view{
  --bg:#17181b; --ink:#e9e9e6; --muted:#9a9a94; --line:#34353b;
  --panel:#1e1f23; --accent:#7fbf9a; --edge:#61626a; --edge-hot:#f97316;
  --sel:#7fbf9a; --bad:#f2b8b5;
  --hive1:rgba(127,191,154,.070); --hive2:rgba(90,170,225,.075);
  --hive3:rgba(170,140,235,.075); --hive4:rgba(240,160,70,.080);
  --hive5:rgba(235,120,170,.070); --hive6:rgba(80,205,205,.075);
  --hive7:rgba(215,205,90,.080); --hive8:rgba(140,140,245,.070);
  --hive9:rgba(245,130,95,.075); --hive10:rgba(190,190,190,.090);
}

html,body{ margin:0; height:100%; background:#fbfbfa; }
@media (prefers-color-scheme: dark){ :root:not([data-theme=light]) body{ background:#17181b; } }
:root[data-theme=dark] body{ background:#17181b; }

.colony-view{ position:absolute; inset:0; background:var(--bg); color:var(--ink);
  font:13px/1.5 ui-sans-serif,system-ui,-apple-system,"Segoe UI",sans-serif;
  cursor:grab; }            /* the empty canvas pans */
.colony-view.panning{ cursor:grabbing; }
.colony-view svg.stage{ width:100%; height:100%; display:block; touch-action:none; }
/* A drag over a picture is never a text selection. Without this the pointer
   sequence on a trackpad can turn into a native selection mid-gesture and the
   `pointerup` never reaches the hook -- the box then hangs on the cursor. */
.colony-view{ user-select:none; -webkit-user-select:none; }
.colony-view .detail{ user-select:text; -webkit-user-select:text; }

.colony-view .node{ cursor:grab; }
.colony-view .node:active{ cursor:grabbing; }
.colony-view .node rect{ stroke-width:1.5; }
.colony-view .node text{ pointer-events:none; user-select:none; }
.colony-view .node text.nm{ fill:#17181b; font:12px system-ui,sans-serif; font-weight:640; }
.colony-view .node text.ty{ fill:#55554f; font:10px system-ui,sans-serif; }

.colony-view path.edge{ fill:none; stroke:var(--edge); stroke-width:1.4;
  marker-end:url(#ar); }
.colony-view path.edge.cond{ stroke-dasharray:7 4; }
.colony-view path.edge.hot{ stroke:var(--edge-hot); stroke-width:2.3; marker-end:url(#arh); }
.colony-view path.edge.dim{ opacity:.10; }
.colony-view path.edge-hit{ fill:none; stroke:transparent; stroke-width:14; cursor:pointer; }

/* `pointer-events:all` is load-bearing, not a tweak: an SVG rect with `fill:none`
   receives pointer events on its STROKE only, so a hive would have been grabbable
   by its 1px dashed border and nothing else. With it, the empty space inside a
   hive is the group's handle — and the cells still win, because the nodes are
   drawn after the hives and therefore get the event first. */
.colony-view .hive rect{ fill:var(--hive1); stroke:var(--line); stroke-dasharray:4 3;
  pointer-events:all; cursor:grab; }
.colony-view .hive.depth-1 rect{ fill:var(--hive1); }
.colony-view .hive.depth-2 rect{ fill:var(--hive2); }
.colony-view .hive.depth-3 rect{ fill:var(--hive3); }
.colony-view .hive.depth-4 rect{ fill:var(--hive4); }
.colony-view .hive.depth-5 rect{ fill:var(--hive5); }
.colony-view .hive.depth-6 rect{ fill:var(--hive6); }
.colony-view .hive.depth-7 rect{ fill:var(--hive7); }
.colony-view .hive.depth-8 rect{ fill:var(--hive8); }
.colony-view .hive.depth-9 rect{ fill:var(--hive9); }
.colony-view .hive.depth-10 rect{ fill:var(--hive10); }
.colony-view .hive text{ fill:var(--muted); font:11px system-ui,sans-serif;
  cursor:grab; user-select:none; }
.colony-view .hive.dragging rect{ stroke:var(--accent); stroke-dasharray:none;
  cursor:grabbing; }
.colony-view .hive.dragging text{ fill:var(--accent); }

/* A cell that takes part in no edge. Disconnected, in almost every case: the
   substrate never deletes a node, so a rewiring leaves its predecessor standing
   here. Dimmed and dashed rather than hidden — a freshly instantiated cell that
   is not wired yet looks identical, and that is worth seeing. */
.colony-view .node.unwired{ opacity:.45; }
.colony-view .node.unwired rect{ stroke-dasharray:5 4; }
.colony-view .node.dim{ opacity:.22; }
.colony-view .node.sel rect{ stroke:var(--sel); stroke-width:2.5; }

.colony-view .legend{ position:absolute; right:.75rem; bottom:.5rem;
  color:var(--muted); font:11px system-ui,sans-serif; }

/* The detail panel. Fixed to the right edge rather than a flex column, because
   the canvas is one absolutely-positioned element inside a LiveView container and
   the SVG has to keep the whole width to scale into. */
.colony-view .detail{ position:absolute; top:0; right:0; width:330px; max-height:100%;
  overflow:auto; padding:12px 14px; box-sizing:border-box;
  background:var(--panel);
  border-left:1px solid var(--line); font:12px system-ui,sans-serif; }
.colony-view .detail .empty{ color:var(--muted); margin:0; }
.colony-view .detail .kv{ margin:0 0 12px; }
.colony-view .detail dt{ color:var(--muted); font-size:10px; text-transform:uppercase;
  letter-spacing:.05em; margin-bottom:2px; }
.colony-view .detail dd{ margin:0; font-family:ui-monospace,SFMono-Regular,Menlo,monospace;
  font-size:11.5px; word-break:break-word; white-space:pre-wrap; color:var(--ink); }
.colony-view .detail .rel{ cursor:pointer; padding:1px 0; }
.colony-view .detail .rel:hover{ color:var(--accent); text-decoration:underline; }
.colony-view .detail .chip{ display:inline-block; padding:1px 7px; border-radius:99px;
  font-size:10px; border:1px solid var(--line); color:var(--muted); }
/* Handing a box back to the layout. Styled as a chip rather than as a form
   control, because it stands where the chip that says "by the layout" stands
   and the two must read as one row of the same panel. */
.colony-view .detail .release{ display:inline-block; padding:1px 7px; border-radius:99px;
  font:inherit; font-size:10px; cursor:pointer; background:none;
  border:1px solid var(--accent); color:var(--accent); }
.colony-view .detail .release:hover{ background:var(--accent); color:var(--bg); }
.colony-view .detail .warn{ margin-top:4px; color:var(--edge-hot); font-size:10.5px;
  font-family:ui-sans-serif,system-ui,sans-serif; white-space:normal; }
@media (max-width:860px){ .colony-view .detail{ display:none; } }

/* Unwired cells: hidden by default, and a button in the legend to bring them
   back. `remove_nodes` and `swap_nodes` disconnect and never delete, so a live
   colony collects disconnected leftovers -- 31 of 123 on the colony this was
   built against. They are part of the truth and they are not part of the flow,
   which is exactly what a toggle is for. */
.colony-view.hide-unwired .node.unwired{ display:none; }
.colony-view .legend .unwired-toggle{ font:inherit; cursor:pointer; padding:0 6px;
  margin-right:6px; border-radius:99px; background:none;
  border:1px solid var(--line); color:var(--muted); }
.colony-view .legend .unwired-toggle:hover{ border-color:var(--accent); color:var(--accent); }
.colony-view:not(.hide-unwired) .legend .unwired-toggle{
  border-color:var(--accent); color:var(--accent); }

/* An OLD TAB. A `<script>` that arrives inside a LiveView morph is not run, so
   a tab open across a template change keeps the browser half it loaded with --
   and every gesture it makes may speak a vocabulary the screen no longer
   accepts. A refused prop write is a receipt to the application and nothing at
   all to the browser, so without this the tab just stops working, silently. */
.colony-view.stale::before{
  content:"this tab is running an older canvas \2014 reload the page";
  position:fixed; z-index:10; left:50%; top:0; transform:translateX(-50%);
  padding:5px 14px 6px; background:#ffffff; color:#b3261e;
  border:1px solid #b3261e; border-top:none; border-radius:0 0 8px 8px;
  font:11.5px/1.4 ui-sans-serif,system-ui,sans-serif; letter-spacing:.02em;
  pointer-events:none;
}

/* Disconnected, and therefore possibly out of date.

   A picture drawn before a colony restart and a live one are pixel-identical, so
   the operator has no way to tell them apart. A rejoin asks for a fresh render
   now instead of taking the cache, but the transport rejoins on its own schedule
   (Phoenix backs off after a socket drop), and until it does, the DOM is whatever
   was last drawn.

   The vendored client already publishes the state: `setContainerClasses` puts
   phx-connected / phx-loading / phx-error / phx-client-error / phx-server-error
   on the `data-phx-main` container, and only after a short delay, so a blink does
   not flash the banner. All that was missing was a stylesheet that reads them. */
[data-phx-main].phx-loading .colony-view,
[data-phx-main].phx-error .colony-view,
[data-phx-main].phx-client-error .colony-view,
[data-phx-main].phx-server-error .colony-view{ opacity:.5; filter:saturate(.35); }
[data-phx-main].phx-loading::after,
[data-phx-main].phx-error::after,
[data-phx-main].phx-client-error::after,
[data-phx-main].phx-server-error::after{
  content:"disconnected \2014 this picture may be out of date";
  position:fixed; z-index:9; left:50%; top:0; transform:translateX(-50%);
  padding:5px 14px 6px; background:#ffffff; color:#b3261e;
  border:1px solid #b3261e; border-top:none; border-radius:0 0 8px 8px;
  font:11.5px/1.4 ui-sans-serif,system-ui,sans-serif; letter-spacing:.02em;
  pointer-events:none;
}
"""
# --- END colony-view.css ---

# The view this cell owns. The display keys a view by (owner, view_id), so this
# string is half of the identity of everything below -- and every component name
# has to start with it, which the display checks and refuses as
# `component_prefix`.
VIEW_ID = "colony-view"
# v1 of the display knows one region and this is it.
REGION = "main"

# The layout constants. The client reads them back off the markup rather than
# keeping a second copy, so a frame computed during a drag is the frame the next
# tick brings back.
NODE_W, NODE_H = 150, 38
# Between two columns of the flow, and between two boxes stacked in one column.
# The horizontal gap is the larger one on purpose: the flow reads left to right,
# and the eye needs the steps along that axis to be the obvious ones.
GAP_X, GAP_Y = 90, 34
# The air between two wrapped rows of columns -- bigger than the gap inside a
# column, so a wrap reads as "the flow continues below" and not as a new branch.
ROW_GAP = 70
# How wide a block may get relative to its height before its columns wrap.
TARGET_RATIO = 2.0
# ...and how wide it has to be before wrapping is considered at all. A four-cell
# chain is a 918-pixel stripe and perfectly readable as one; breaking it into a
# tower to satisfy a ratio makes the flow harder to follow, not easier. Wrapping
# is a remedy for blocks that outgrow a screen, not a rule about shape.
WRAP_MIN_W = 1100
PAD_TOP, PAD_SIDE, PAD_BOT = 30, 24, 24
# How far INSIDE its parent a hive's frame sits. Constant, not a function of
# depth: an ancestor is bigger than its child by this much at every level, which
# is what makes nesting readable without measuring, and what lets the client
# recompute a frame during a drag with the same arithmetic this cell used.
NEST = 18
# How many depths the stylesheet has a tint for. Deeper hives keep the last one.
# Ten, because a real colony gets deep: an assistant's keeper cell sits eight
# levels down, and two hives at different depths sharing a colour is exactly the
# thing the tint exists to prevent.
HIVE_DEPTH_TINTS = 10

# Fill and stroke per cell type.
TYPE_COLOR = {
    "llm": ("#d6e4ff", "#4a6fa5"),
    "code": ("#e8e8e8", "#8a8a8a"),
    "file": ("#e8e8e8", "#8a8a8a"),
    "edit": ("#e8e8e8", "#8a8a8a"),
    "store": ("#ffe6cc", "#d79b00"),
    "proxy": ("#d5e8d4", "#82b366"),
    "timer": ("#fff2cc", "#d6b656"),
    "web": ("#dae8fc", "#6c8ebf"),
    "web_fetch": ("#f8cecc", "#b85450"),
    "web_search": ("#f8cecc", "#b85450"),
    "bash": ("#e1d5e7", "#9673a6"),
    "harness": ("#e1d5e7", "#9673a6"),
    "mcp": ("#e1d5e7", "#9673a6"),
    "vault": ("#f5f5f5", "#666666"),
    "subcolony": ("#dae8fc", "#6c8ebf"),
}
DEFAULT_COLOR = ("#ffffff", "#999999")

CLIENT_ID = hashlib.sha1(
    (CLIENT_JS + CLIENT_CSS).encode("utf-8")).hexdigest()[:12]


# ---------------------------------------------------------------------------
# The component library. Data, not code: a display's `component.define` takes a
# template written in the closed four-form language (a prop, a raw prop, the
# children slot and a conditional) and nothing else, and it parses it at
# DEFINITION time -- so a mistake here is answered to this cell, not rendered as
# a blank area on somebody's screen.
#
# The two client props are raw on purpose, and the schema types them `"html"`,
# which is what makes them raw: a raw prop whose schema does not say `"html"` is
# escaped instead, and a stylesheet rendered escaped is a page with no style at
# all. Everything else is escaped by default -- a cell path is database content
# and a name carrying a script tag must not be able to close one.

SHELL_TEMPLATE = (
    '<div class="colony-view hide-unwired" id="colony-view" phx-hook="ColonyView"'
    ' data-title="{{title}}"'
    ' data-client="{{client}}"'
    ' data-nw="{{nw}}" data-nh="{{nh}}" data-pad-side="{{pad_side}}"'
    ' data-pad-top="{{pad_top}}" data-pad-bot="{{pad_bot}}" data-nest="{{nest}}">'
    "<style>{{&client_css}}</style>"
    "<script>{{&client_js}}</script>"
    '<svg class="stage" xmlns="http://www.w3.org/2000/svg" viewBox="{{viewbox}}"'
    ' preserveAspectRatio="xMidYMid meet">'
    # The arrowheads. The stylesheet asks for these by id, and the ids carry the
    # view's own prefix: a display may hold more than one view, and two of them
    # defining a marker called `ar` would be one document with two answers to the
    # same name.
    "<defs>"
    '<marker id="colony-view-ar" viewBox="0 0 10 10" refX="9" refY="5"'
    ' markerWidth="6.5" markerHeight="6.5" orient="auto-start-reverse">'
    '<path d="M0,0 L10,5 L0,10 z" fill="var(--edge)"/></marker>'
    '<marker id="colony-view-arh" viewBox="0 0 10 10" refX="9" refY="5"'
    ' markerWidth="6.5" markerHeight="6.5" orient="auto-start-reverse">'
    '<path d="M0,0 L10,5 L0,10 z" fill="var(--edge-hot)"/></marker>'
    "</defs>"
    # The camera transform lives on this group and is written by the client alone.
    # It starts at identity: the `viewBox` above already frames the whole drawing,
    # so a fresh load is legible before any script runs.
    '<g class="viewport">{{children}}</g>'
    "</svg>"
    # Somewhere to say what was clicked. Without it the whole selection idea is
    # invisible and an edge's condition stays a string in an attribute nobody can
    # read. The panel is filled by the client -- this cell renders the frame, not
    # the answer, because what is selected is a client-side fact.
    '<aside class="detail"><p class="empty">Click a cell or an edge.</p></aside>'
    # The toggle, and the count that makes it worth reaching for. UNWIRED means
    # a cell that takes part in no edge at all -- in this substrate almost
    # always a cell a rewiring left standing, because `remove_nodes` and
    # `swap_nodes` DISCONNECT and never delete (`docs/rewiring.en.md`: getting
    # rid of a registry row is an operator action with the colony stopped). A
    # live colony therefore accumulates them: 31 of 123 cells on the one this
    # was built against, 13 of them in a single hive. Hidden by default, because
    # the picture is for looking at what the colony DOES.
    '<div class="legend"><button class="unwired-toggle" type="button">'
    '{{unwired}} unwired</button> {{cells}} cells, {{hives}} hives,'
    ' {{edges}} edges</div>'
    "</div>"
)

HIVE_TEMPLATE = (
    # `depth-N` is what the stylesheet tints. Clamped, because a tree deeper than
    # the palette should keep the deepest colour rather than fall back to none --
    # a hive with no tint would read as "not a hive".
    '<g class="hive depth-{{depth}}" data-hive="{{path}}" data-oid="{{oid}}">'
    '<rect x="{{x}}" y="{{y}}" width="{{w}}" height="{{h}}" rx="10"/>'
    '<text x="{{tx}}" y="{{ty}}">{{name}}</text>'
    "</g>"
)

EDGE_TEMPLATE = (
    # Two paths per edge and both matter. The thin one is the line; the fat
    # transparent one is what a mouse can hit -- an edge is 1.4 px wide, and
    # "click an edge to read its condition" is not a feature you can offer on
    # 1.4 px.
    #
    # `cond` is a CLASS and not only a data attribute: the stylesheet dashes a
    # conditional edge, and half of a real colony's edges carry a condition. A
    # picture that draws them like the rest hides exactly what an operator came to
    # look for. The condition text and the modifier ride along so that clicking
    # the edge can show them in full without another round trip.
    '<g class="edge-g">'
    '<path class="edge{{#if cond}} cond{{/if}}" data-edge="{{eid}}"'
    ' data-from="{{from}}" data-to="{{to}}" data-lane="{{lane}}"'
    ' data-cond="{{cond}}" data-mod="{{mod}}" data-vlane="{{vlane}}"/>'
    '<path class="edge-hit" data-edge="{{eid}}"/>'
    "</g>"
)

NODE_TEMPLATE = (
    # `data-node` is the colony PATH -- what an edge names, and what the client's
    # frame arithmetic splits on. `data-oid` is the OBJECT id, which is what a
    # drag writes to. Keeping them apart is what lets a hive frame and a cell of
    # the same path both exist as objects.
    #
    # TWO translates, and the split is the whole of GH #544. The first is the
    # FLOW's, rewritten on every tick, and the second is the HAND's, written by
    # a drag and never by this cell. SVG composes a transform list left to
    # right, so the pair needs no arithmetic anywhere -- which matters, because
    # the component language has none: it substitutes props and nothing else.
    #
    # A hand's offset is therefore relative to wherever the flow currently puts
    # the cell, so it travels with its hive instead of being left behind by the
    # next re-rank. That is what [#170](https://github.com/mmeyerlein/meclaw/issues/170)
    # removed -- an anchor measured against a layout that every arriving cell
    # re-derives -- put back the one way that does not walk off: the delta is
    # against the cell's OWN spot, not against a hive anchor.
    #
    # `data-pinned` is the marker, in the markup, because the client has to know
    # whether there is anything to release before it draws the detail panel.
    # The MARKER is what says a hand was here; a coordinate says nothing, and
    # reading one as a pin is what put 103 of 104 boxes on a stranger's spot
    # (#544). `canvy@2.1.8` learnt this first.
    #
    # `unwired` says the cell takes part in no edge at all. In this substrate that
    # is almost always a DISCONNECTED cell -- removing a node drops every edge and
    # keeps the node (no-delete), so a rewiring leaves its predecessor standing in
    # the picture looking exactly like a live one. It is a heuristic and not an
    # answer (the graph endpoint does not report activity), which is why it dims
    # rather than hides: a cell instantiated a second ago and not yet wired looks
    # the same, and that is a state worth seeing too.
    '<g class="node{{#if unwired}} unwired{{/if}}" data-node="{{path}}"'
    ' data-oid="{{oid}}" data-pinned="{{pinned}}"'
    ' transform="translate({{x}},{{y}}) translate({{hand}})">'
    '<rect width="{{w}}" height="{{h}}" rx="6" fill="{{fill}}" stroke="{{stroke}}"/>'
    '<text class="nm" x="8" y="16">{{name}}</text>'
    '<text class="ty" x="8" y="30">{{type}}</text>'
    "</g>"
)


def components():
    """The four component definitions, in the order they are defined.

    Every name starts with the view id, which is not a convention: the display
    refuses a component whose name does not, and says `component_prefix`. Two
    views on one display therefore cannot collide on a name however they were
    written.
    """
    return [
        {
            "name": "colony-view-shell",
            "template": SHELL_TEMPLATE,
            "prop_schema": {
                "title": "text",
                "client": "text",
                "viewbox": "text",
                "cells": "int",
                "hives": "int",
                "edges": "int",
                "unwired": "int",
                "nw": "int",
                "nh": "int",
                "pad_side": "int",
                "pad_top": "int",
                "pad_bot": "int",
                "nest": "int",
                "client_css": "html",
                "client_js": "html",
            },
            "editable": [],
            "layer": "content",
        },
        {
            "name": "colony-view-hive",
            "template": HIVE_TEMPLATE,
            "prop_schema": {
                "path": "text",
                "oid": "text",
                "name": "text",
                "depth": "int",
                "x": "int",
                "y": "int",
                "w": "int",
                "h": "int",
                "tx": "int",
                "ty": "int",
            },
            # A frame is DERIVED from the cells it holds -- and since 1.0.1 the
            # browser is the only half that can see where they ended up, because
            # a hand's offset lives in the display and this cell never learns it.
            # So the browser writes the derived rectangle back, and the store
            # says what the screen shows instead of a third, wrong geometry
            # (GH #544 measured 96 of 104 cells outside the frame the store
            # held). NOT kept: the flow says the rectangle again on every tick,
            # and the browser corrects it again in the same tick if a hand has
            # moved something. With no browser open, the flow's frame stands,
            # which is the right answer for a reader that is not looking at an
            # arrangement.
            "editable": ["x", "y", "w", "h"],
            "layer": "content",
        },
        {
            "name": "colony-view-edge",
            "template": EDGE_TEMPLATE,
            "prop_schema": {
                "eid": "text",
                "from": "text",
                "to": "text",
                "lane": "int",
                "cond": "text",
                "vlane": "text",
                "mod": "text",
            },
            "editable": [],
            "layer": "content",
        },
        {
            "name": "colony-view-node",
            "template": NODE_TEMPLATE,
            "prop_schema": {
                "path": "text",
                "oid": "text",
                "name": "text",
                "type": "text",
                "x": "int",
                "y": "int",
                # The hand's half: ONE prop, and that is not cosmetic. A
                # browser writes a prop at a time and the display diffs a prop
                # at a time, so an offset spelled as two props reaches the page
                # as two pictures -- and the frames, derived from where the
                # boxes are, are derived once from the half-moved one. Measured
                # in a browser under GH #544: three different rectangles were
                # painted for one drag, the middle one 971 wide and still 92
                # high. `"dx,dy"` is one write, one diff, one picture. SVG
                # composes a transform list, so `translate({{hand}})` needs no
                # arithmetic and no parsing on the way in.
                "hand": "text",
                "pinned": "text",
                "w": "int",
                "h": "int",
                "fill": "text",
                "stroke": "text",
                "unwired": "text",
            },
            # The declaration that makes a drag possible at all, and the whole
            # authorisation model with it. The display checks it against the
            # COMPONENT, never against the message: a browser says what it wants
            # changed, and the component says what may be. A prop that is not on
            # the list is refused with `not_editable` and nothing is written.
            #
            # `keep` is the other half and it lives on the tree entry, not here:
            # `editable` says who may write these props, `keep` says this cell
            # stops overwriting them once the object exists.
            #
            # `x`/`y` are NOT on this list any more and are not kept either: the
            # flow owns them, and it owns them on every tick, which is the whole
            # repair of GH #544. What a hand writes is `hand` and the marker.
            "editable": ["hand", "pinned"],
            "layer": "content",
        },
    ]


# ---------------------------------------------------------------------------
# The tree the colony graph describes


def hive_of(path):
    """The parent path of a cell, without its leading slash. "" at the root."""
    p = path.strip("/")
    return p.rsplit("/", 1)[0] if "/" in p else ""


def element_of(path, hive):
    """Which element of `hive` a cell belongs to: itself, or the child hive.

    A hive is laid out over ITS OWN elements -- the cells that live directly in it
    and the child hives, each as one block. So an edge to a cell three levels
    down is, from here, an edge to the child hive that contains it. Projecting
    edges this way is what makes the flow visible at every level instead of only
    at the leaves.
    """
    if hive:
        if not path.startswith(hive + "/"):
            return None
        rest = path[len(hive) + 1:]
    else:
        rest = path
    head = rest.split("/", 1)[0]
    return (hive + "/" + head) if hive else head


def back_edges(ids, pairs):
    """The edges that point backwards in a depth-first walk.

    A colony graph is full of cycles by design -- every request/reply pair is one,
    and a picture that tries to rank a cycle either spins or produces nonsense.
    So the walk names the edges that close a loop, ranking ignores them, and they
    are still DRAWN: they are precisely the "and back up again" lanes, and they
    read correctly once the forward flow has decided the columns.
    """
    out = {}
    for a, b in pairs:
        out.setdefault(a, []).append(b)
    state = {}
    back = set()
    for root in ids:
        if state.get(root):
            continue
        state[root] = 1
        stack = [(root, sorted(set(out.get(root, [])), reverse=True))]
        while stack:
            node, rest = stack[-1]
            if not rest:
                state[node] = 2
                stack.pop()
                continue
            nxt = rest.pop()
            if state.get(nxt) == 1:
                back.add((node, nxt))
            elif not state.get(nxt):
                state[nxt] = 1
                stack.append((nxt, sorted(set(out.get(nxt, [])), reverse=True)))
    return back


def columns_of(ids, pairs):
    """A column per element: one to the RIGHT of everything that feeds it.

    Left to right is the direction a turn travels, so it is the direction the
    picture puts it in. Longest path from the sources, on the graph with its back
    edges removed -- which terminates, because what is left is a DAG.
    """
    back = back_edges(ids, pairs)
    fwd = [(a, b) for (a, b) in pairs if a != b and (a, b) not in back]
    incoming = {i: [] for i in ids}
    for a, b in fwd:
        incoming[b].append(a)
    col = {i: 0 for i in ids}
    for _ in range(len(ids) + 1):
        changed = False
        for i in ids:
            want = max([col[s] + 1 for s in incoming[i]], default=0)
            if want != col[i]:
                col[i] = want
                changed = True
        if not changed:
            break
    return col, fwd


def spine_of(ids, fwd, col):
    """The elements on a LONGEST chain through the flow.

    They are drawn on one line, at the top of their column, so the main current
    is a straight run across the picture and everything else hangs off it. That
    is the whole difference between a diagram you read and a field of boxes: a
    reader needs one line to follow before they can see what branches off it.
    """
    outgoing = {i: [] for i in ids}
    for a, b in fwd:
        outgoing[a].append(b)
    height = {i: 0 for i in ids}
    for i in sorted(ids, key=lambda k: -col[k]):     # sinks first
        height[i] = max([height[j] + 1 for j in outgoing[i]], default=0)
    longest = max([col[i] + height[i] for i in ids], default=0)
    return {i for i in ids if col[i] + height[i] == longest}


def order_columns(cols, fwd, spine):
    """The vertical order inside each column: spine on top, branches below.

    Barycentre passes -- each element wants to sit level with its neighbours in
    the previous column -- alternating forwards and backwards, which is the
    standard way to pull crossings out of a layered drawing. The spine is pinned
    to the top row throughout: a main line that wanders up and down between
    columns is exactly as hard to follow as no main line at all.
    """
    order = {c: sorted(members) for c, members in cols.items()}
    nbr_in, nbr_out = {}, {}
    for a, b in fwd:
        nbr_in.setdefault(b, []).append(a)
        nbr_out.setdefault(a, []).append(b)
    rank = {}
    for c in order:
        order[c] = sorted(order[c], key=lambda i: (0 if i in spine else 1, i))
        for k, i in enumerate(order[c]):
            rank[i] = k

    def barycentre(i, src):
        near = [rank[n] for n in src.get(i, []) if n in rank]
        return sum(near) / float(len(near)) if near else rank[i]

    for step in range(4):
        src = nbr_in if step % 2 == 0 else nbr_out
        for c in sorted(order, reverse=bool(step % 2)):
            order[c] = sorted(
                order[c],
                key=lambda i: (0 if i in spine else 1, barycentre(i, src), i),
            )
            for k, i in enumerate(order[c]):
                rank[i] = k
    return order


def hive_tree(ids):
    """The hive paths in the picture, as parent -> sorted children.

    Every ancestor of every cell is a hive, whether or not it holds a cell of its
    own: a cell three levels down makes a hive of each prefix above it. Drawing
    only the direct parent was the defect reported as "hive in hive does not work
    properly yet" -- a colony is a tree, and a picture that shows only its leaves'
    parents shows none of it.
    """
    hives = set()
    for i in ids:
        parts = i.split("/")[:-1]          # every prefix above the cell itself
        for n in range(1, len(parts) + 1):
            hives.add("/".join(parts[:n]))
    kids = {"": []}
    for h in hives:
        kids.setdefault(h, [])
        parent = h.rsplit("/", 1)[0] if "/" in h else ""
        kids.setdefault(parent, []).append(h)
    for k in kids:
        kids[k] = sorted(kids[k])
    return kids


def split_tall_columns(order, size):
    """Break a column that has grown into a tower into side-by-side columns.

    Everything that shares a flow rank shares a column, and a colony has plenty
    of elements with no edge between them at all -- six independent hives all land
    in the first column and stack into a strip, which is precisely the shape this
    view was reported unusable for the first time. Nothing about that is a flow
    statement: they are peers, so they belong beside each other.

    The order inside the column is preserved, so the spine stays first and the
    barycentre work above is not undone.
    """
    out = []
    for c in sorted(order):
        members = order[c]
        wide = max(size[i][0] for i in members)
        tall = sum(size[i][1] for i in members) + GAP_Y * (len(members) - 1)
        if len(members) < 2 or tall <= TARGET_RATIO * wide:
            out.append(members)
            continue
        k = (TARGET_RATIO * tall / float(wide)) ** 0.5
        k = min(len(members), max(1, int(k) + (1 if k > int(k) else 0)))
        per = len(members) // k + (1 if len(members) % k else 0)
        for start in range(0, len(members), per):
            out.append(members[start:start + per])
    return out


def place_columns(order, size):
    """Put the ordered columns on the plane, wrapping when the run gets too long.

    A colony is a long thin thing: a four-cell chain drawn as pure left-to-right
    is a 918x92 stripe. Stack a few of those and the whole picture came out
    8232x848 -- a ratio of ten to one, which a screen can only show by shrinking
    every label past reading. The first version of this view failed the other way
    round, as one tall column; both are the same mistake, which is letting one
    axis carry all of the structure.

    So the columns wrap like text: left to right along a row, then down and back
    to the left. Within a row the flow still reads the way it runs, and a chain
    longer than the row is broken between columns -- never inside one, so no step
    of the flow is ever split.

    `TARGET_RATIO` is what "readable" means here: a block roughly twice as wide as
    it is tall, at every depth, so the whole nest stays close to the shape of the
    screen it is shown on.
    """
    groups = split_tall_columns(order, size)
    cols = list(range(len(groups)))
    order = {c: groups[c] for c in cols}
    col_w = {c: max(size[i][0] for i in order[c]) for c in cols}
    col_h = {
        c: sum(size[i][1] for i in order[c]) + GAP_Y * (len(order[c]) - 1)
        for c in cols
    }
    total_w = sum(col_w.values()) + GAP_X * max(0, len(cols) - 1)
    tallest = max(col_h.values())

    # How many rows to break into: with `k` rows the block is roughly
    # `total_w / k` wide and `k * tallest` high, and asking those to sit at the
    # target ratio gives the square root below. Deriving the limit from the summed
    # column AREA instead looks similar and is wrong -- a chain of flat columns has
    # almost no area, so every block became a tower, which is the same unreadable
    # shape this started from.
    limit = total_w
    if total_w > WRAP_MIN_W and total_w > TARGET_RATIO * tallest:
        rows = max(1, int(round((total_w / float(TARGET_RATIO * tallest)) ** 0.5)))
        limit = max(total_w / float(rows), max(col_w.values()))

    place = {}
    x, y_row, row_h = PAD_SIDE, PAD_TOP, 0
    for c in cols:
        if x > PAD_SIDE and x - PAD_SIDE + col_w[c] > limit:
            y_row += row_h + ROW_GAP
            x, row_h = PAD_SIDE, 0
        y = y_row
        for i in order[c]:
            place[i] = (x, y)
            y += size[i][1] + GAP_Y
        x += col_w[c] + GAP_X
        row_h = max(row_h, col_h[c])
    return place


def layout_block(hive, kids, ids_by_hive, pairs):
    """One hive and everything under it, laid out relative to its own frame.

    The same shape at every depth, which is what makes nesting work: a hive's
    elements are its own cells AND its child hives -- a child is one block with
    one size, and the parent never looks inside it. Elements are placed left to
    right by flow, stacked top to bottom within a column, spine first.

    Returns `(positions, width, height)` with (0, 0) at the frame's top-left
    corner, so a parent can place the block by its frame and nothing else.
    """
    children = kids.get(hive, [])
    sub = {c: layout_block(c, kids, ids_by_hive, pairs) for c in children}

    size = {i: (NODE_W, NODE_H) for i in ids_by_hive.get(hive, [])}
    for c in children:
        # A child's element is its frame plus the inset it will sit at, so no two
        # frames can touch however the columns fall.
        size[c] = (sub[c][1] + 2 * NEST, sub[c][2] + 2 * NEST)
    ids = sorted(size)
    if not ids:
        return {}, 0, 0

    projected = set()
    for a, b in pairs:
        ea, eb = element_of(a, hive), element_of(b, hive)
        if ea in size and eb in size and ea != eb:
            projected.add((ea, eb))

    col, fwd = columns_of(ids, sorted(projected))
    spine = spine_of(ids, fwd, col)
    cols = {}
    for i in ids:
        cols.setdefault(col[i], []).append(i)
    order = order_columns(cols, fwd, spine)

    place = place_columns(order, size)

    pos = {}
    for i in ids:
        px, py = place[i]
        if i in sub:
            for k, (rx, ry) in sub[i][0].items():
                pos[k] = (px + NEST + rx, py + NEST + ry)
        else:
            pos[i] = (px, py)

    # The frame: this hive's own cells padded, plus every child's frame grown by
    # the nesting inset. The same union `hive_frames` computes from the finished
    # positions, so the block reports exactly the rectangle that will be drawn.
    boxes = []
    own = [pos[i] for i in ids_by_hive.get(hive, [])]
    if own:
        boxes.append(box_of(own))
    for c in children:
        cx, cy = place[c]
        boxes.append((cx, cy, sub[c][1] + 2 * NEST, sub[c][2] + 2 * NEST))
    x0 = min(b[0] for b in boxes)
    y0 = min(b[1] for b in boxes)
    w = max(b[0] + b[2] for b in boxes) - x0
    h = max(b[1] + b[3] for b in boxes) - y0
    return {i: (p[0] - x0, p[1] - y0) for i, p in pos.items()}, w, h


def flow_layout(nodes, edges):
    """Where every box goes: the whole picture, computed from the graph alone.

    A function of the WHOLE node set -- column ranks, barycentre order and block
    sizes all change when a single cell arrives. Deterministic: same graph, same
    picture, so a changed drawing means a changed colony.

    Nothing a person did is expressed against this space, and that is the point.
    A stored point measured against a layout that every arriving cell re-derives
    walks off as soon as one cell is instantiated -- measured once at twelve of
    nineteen hand-placed frames
    ([#170](https://github.com/mmeyerlein/meclaw/issues/170)). Here a hand-placed
    box is not stored against the flow at all: the display keeps the browser's own
    absolute `x`/`y` and this cell never overwrites them, so the flow is only ever
    consulted for a box nobody has touched.
    """
    ids_by_hive = {}
    for n in nodes:
        ids_by_hive.setdefault(hive_of(n["id"]), []).append(n["id"])
    kids = hive_tree([n["id"] for n in nodes])
    pairs = sorted({(e["from"], e["to"]) for e in edges})

    pos, _, _ = layout_block("", kids, ids_by_hive, pairs)
    return pos


def box_of(points):
    """The padded frame around a set of cell positions."""
    xs = [p[0] for p in points]
    ys = [p[1] for p in points]
    x = min(xs) - PAD_SIDE
    y = min(ys) - PAD_TOP
    return (x, y, (max(xs) + NODE_W + PAD_SIDE) - x, (max(ys) + NODE_H + PAD_BOT) - y)


def hive_frames(pos):
    """One rectangle per hive: `{hive: (x, y, w, h)}`.

    Its own cells, padded, unioned with every child's frame grown by the nesting
    inset. Recursive, so a hive that holds nothing but sub-hives still gets a
    frame, and an ancestor is strictly bigger than its child at every level.

    Derived and never stored, which is what makes a cell dragged out of a crowd
    GROW its hive -- and every hive ABOVE it -- instead of being stranded outside
    a stale rectangle. The client computes the same union during a drag, from the
    same constants, so the frames follow the cursor instead of waiting for the
    next tick.
    """
    kids = hive_tree(list(pos))
    own = {}
    for i in pos:
        own.setdefault(hive_of(i), []).append(pos[i])
    out = {}

    def rect(h):
        if h in out:
            return out[h]
        boxes = [box_of(own[h])] if own.get(h) else []
        for c in kids.get(h, []):
            cx, cy, cw, ch = rect(c)
            boxes.append((cx - NEST, cy - NEST, cw + 2 * NEST, ch + 2 * NEST))
        if not boxes:
            return (0, 0, 0, 0)
        x = min(b[0] for b in boxes)
        y = min(b[1] for b in boxes)
        out[h] = (x, y, max(b[0] + b[2] for b in boxes) - x,
                  max(b[1] + b[3] for b in boxes) - y)
        return out[h]

    for h in sorted(kids, key=lambda k: -k.count("/")):
        if h:
            rect(h)
    return out


def hive_of_cell_frames(pos):
    """Kept as the one sentence GH #544 ended on, so the next reader finds it.

    1.0.1 shipped a bound: a hand's offset was trimmed to its own hive's frame,
    which made the four target points of #544 hold by construction. It was measured on a
    real screen and it was unusable -- a hive's frame is shrink-wrapped around
    its cells, so at the zoom a whole colony is looked at, a box had 85 x 15
    screen pixels of travel before it hit a wall. The gesture read as broken,
    and a constraint a person experiences as a broken gesture is not a
    constraint, it is a bug.

    So the bound is gone. A hand may put a box anywhere, the frames follow the
    boxes (the browser re-derives them on every drag frame and on every tick,
    and writes the result back), and what keeps the picture honest is the
    MARKER, not a wall: nothing moves that a hand did not move, every box that
    moved says so, and the detail panel hands it back to the layout.

    What is therefore NOT guaranteed any more, and is written down rather than
    hoped away: an arrangement can put two frames over each other, because a
    hand asked for it. The durable answer is for the app to REMEMBER the
    arrangement -- a store in the app hive and an event lane back from the
    screen, so the flow can pack around what a hand did instead of being blind
    to it. That is a build, not a repair, and it is filed as its own issue.
    """
    return hive_frames(pos)


def hive_key(path):
    """The object key of a hive's rectangle. Same reasoning as `node_key`."""
    return "h." + path.replace("/", "~")


def node_key(path):
    """The object key of a cell's box: its colony path, and never its slot.

    An object id is minted by the display from the tree entry's `key` where
    there is one and from the child INDEX where there is not, and an index is a
    slot: the picture writes its hives, then its edges, then its cells, so one
    edge more moves every cell's index by one. Under GH #544 that is what turned
    `keep` into a lie -- the kept prop stayed with the slot while the cell that
    had asked for it moved on, and 103 of 104 boxes ended up wearing a position
    computed for somebody else. A key that is the path makes a kept prop follow
    the cell, which is the only thing `keep` can usefully mean.

    The `n.` prefix keeps the key out of the index's language: a cell whose path
    is `5` must not be able to name the object of the sixth child.
    """
    return "n." + path.replace("/", "~")


def hive_boxes(pos):
    """The hive rectangles, ready to become components."""
    frames = hive_frames(pos)
    boxes = []
    for h in sorted(frames):
        x, y, w, ht = frames[h]
        boxes.append(
            {
                "id": h,
                "name": h.rsplit("/", 1)[-1],
                "depth": h.count("/") + 1,
                "x": x,
                "y": y,
                "w": w,
                "h": ht,
            }
        )
    return boxes


def edge_lanes(edges):
    """A lane number per edge, centred within its endpoint pair.

    Two passes: number the edges inside each unordered pair, then shift the
    bundle so it straddles the centre line. Parallel edges between one pair then
    stay apart instead of overlapping.
    """
    groups = {}
    for e in edges:
        key = tuple(sorted((e["from"], e["to"])))
        groups.setdefault(key, []).append(e)
    for members in groups.values():
        n = len(members)
        for k, e in enumerate(members):
            e["lane"] = k - (n - 1) // 2
    return edges


def frame(nodes, hives):
    """The `viewBox`: the whole drawing, plus a margin.

    Without it the SVG shows the top-left corner of a canvas that is thousands of
    pixels tall, and since the element is exactly the size of its container there
    is nothing to scroll. Reported as "not usable", and rightly. With a viewBox
    the browser scales the whole arrangement into the frame on its own, before any
    JavaScript runs -- so the canvas is legible even if the client never loads.

    Pan and zoom then ride on TOP of this as the camera transform, which is why
    the frame is derived from the content and never from the camera.
    """
    xs = [n["x"] for n in nodes] + [h["x"] for h in hives]
    ys = [n["y"] for n in nodes] + [h["y"] for h in hives]
    if not xs:
        return (0, 0, NODE_W, NODE_H)
    right = max([n["x"] + NODE_W for n in nodes] + [h["x"] + h["w"] for h in hives])
    bottom = max([n["y"] + NODE_H for n in nodes] + [h["y"] + h["h"] for h in hives])
    x0 = min(xs) - PAD_SIDE
    y0 = min(ys) - PAD_TOP
    return (x0, y0, right - x0 + PAD_SIDE, bottom - y0 + PAD_BOT)


# ---------------------------------------------------------------------------
# Saying it as a view


def wrapper_id(owner):
    """The id the display will give this view's root, or "" when it cannot be known.

    Deterministic and specified: the display keys a view by (owner, view_id) and
    names its root after both, with the owner's slashes written as tildes. The
    owner is this cell's own path, which arrives as the target of the message
    being handled -- the substrate stamps an emission's `reply_to` with exactly
    that path, so the two agree by construction.

    Computing it here is a deliberate borrowing of somebody else's scheme, and it
    buys one thing: the browser has to name an object to move it, a component
    template is the only channel from this cell to the browser, and a prop is the
    only way to fill one. Without it a drag would have nothing to write to.
    """
    if not owner:
        return ""
    return "view." + owner.replace("/", "~") + "." + VIEW_ID


def child_id(wrapper, key):
    """The object id the display will mint for a keyed child of this view.

    A view's tree is handed to the display's `add_tree` at the WRAPPER, and the
    root of that tree is this cell's shell -- so the shell is `<wrapper>/0` and
    everything this cell calls a child of the shell is one level below that.
    Until 1.0.1 this function said `<wrapper>/<index>` and skipped the shell's
    level entirely, so every `data-oid` in the picture named an object that does
    not exist: a drag wrote nowhere, and nothing on this screen could be moved
    at all. `gh544_the_flow_reaches_the_screen` mints the ids with the display's
    own `add_tree` and compares, so the two cannot drift apart again.
    """
    return (wrapper + "/0/" + key) if wrapper else ""


def picture(graph):
    """The nodes and the edges this graph asks for, drawing order not yet applied."""
    edges = []
    for e in graph.get("edges") or []:
        if not isinstance(e, dict):
            continue
        mod = e.get("modifier")
        edges.append({
            "id": str(e.get("id") or ""),
            "from": str(e.get("from") or "").strip("/"),
            "to": str(e.get("to") or "").strip("/"),
            "cond": str(e.get("condition") or ""),
            # GH #559: the lane a v-lane was declared on. Empty for every
            # ordinary edge, which is what the browser half tests on.
            "vlane": str(e.get("lane") or ""),
            "mod": json.dumps(mod, sort_keys=True) if mod else "",
        })
    edges = edge_lanes(edges)
    wired = {e["from"] for e in edges} | {e["to"] for e in edges}
    nodes = []
    for n in graph.get("nodes") or []:
        if not isinstance(n, dict):
            continue
        path = str(n.get("path") or "").strip("/")
        if not path:
            continue
        nodes.append({
            "id": path,
            "name": path.rsplit("/", 1)[-1],
            "type": str(n.get("cell_type") or ""),
            "unwired": path not in wired,
        })
    return nodes, edges


def content(graph, owner):
    """The component tree: one shell, then the hives, the edges and the cells.

    The order of `children` IS the drawing order -- the display hands out `ord`
    from the index -- and a line under a box reads as a line while a box under a
    line reads as a mistake.
    """
    nodes, edges = picture(graph)
    pos = flow_layout(nodes, [{"from": e["from"], "to": e["to"]} for e in edges])
    for n in nodes:
        n["x"], n["y"] = pos.get(n["id"], (0, 0))
    # The frames -- and the viewBox with them -- are computed over the cells the
    # picture SHOWS, which by default is the wired ones. A hive whose members
    # are all disconnected leftovers would otherwise spread its rectangle over
    # a part of the canvas nobody is looking at. Turning the toggle on re-derives
    # both in the browser, which is where the toggle lives.
    shown = {i: p for i, p in pos.items()
             if i not in {n["id"] for n in nodes if n["unwired"]}} or pos
    hives = hive_boxes(shown)
    box = frame([n for n in nodes if not n["unwired"]] or nodes, hives)
    wrapper = wrapper_id(owner)

    children = []
    for h in hives:
        children.append({
            "component": "colony-view-hive",
            "props": {
                "path": h["id"],
                "oid": child_id(wrapper, hive_key(h["id"])),
                "name": h["name"],
                "depth": max(1, min(HIVE_DEPTH_TINTS, h["depth"])),
                "x": h["x"],
                "y": h["y"],
                "w": h["w"],
                "h": h["h"],
                "tx": h["x"] + 8,
                "ty": h["y"] + 18,
            },
            "key": hive_key(h["id"]),
        })
    for e in edges:
        children.append({
            "component": "colony-view-edge",
            "props": {
                "eid": e["id"],
                "from": e["from"],
                "to": e["to"],
                "lane": e.get("lane", 0),
                "cond": e["cond"],
                "mod": e["mod"],
                "vlane": e["vlane"],
            },
        })
    for n in nodes:
        fill, stroke = TYPE_COLOR.get(n["type"], DEFAULT_COLOR)
        children.append({
            "component": "colony-view-node",
            "props": {
                "path": n["id"],
                "oid": child_id(wrapper, node_key(n["id"])),
                "name": n["name"],
                "type": n["type"],
                "x": n["x"],
                "y": n["y"],
                # Written on a create and never again -- but written as the
                # NEUTRAL value, so a picture nobody has arranged is exactly the
                # flow's picture and nothing else.
                "hand": "0,0",
                "pinned": "",
                "w": NODE_W,
                "h": NODE_H,
                "fill": fill,
                "stroke": stroke,
                # A prop the conditional has to read as absent when it is false,
                # and the empty string is how this template language spells that.
                "unwired": "1" if n["unwired"] else "",
            },
            # The identity of this box, and it is the CELL rather than the slot
            # it happens to occupy in `children`. Everything below depends on
            # it: an id that moves when an edge is added hands the kept props to
            # whichever cell inherits the slot (GH #544).
            "key": node_key(n["id"]),
            # The two props a hand owns. On an object the display already
            # holds they are left out of the update, and an update merges per
            # key, so the browser's value stands and an arrangement survives
            # every tick. `x` and `y` are deliberately NOT among them: the flow
            # owns where a cell is, on every tick, and what a hand contributes
            # is the offset beside it.
            "keep": ["hand", "pinned"],
        })

    return {
        "component": "colony-view-shell",
        "props": {
            "title": str(graph.get("scope") or "/"),
            "viewbox": "%d %d %d %d" % box,
            "cells": len(nodes),
            "hives": len(hives),
            "edges": len(edges),
            "unwired": sum(1 for n in nodes if n["unwired"]),
            "nw": NODE_W,
            "nh": NODE_H,
            "pad_side": PAD_SIDE,
            "pad_top": PAD_TOP,
            "pad_bot": PAD_BOT,
            "nest": NEST,
            # Which browser half this picture was written by. A `<script>` that
            # arrives inside a LiveView morph is NOT executed, so a tab that has
            # been open across a template change keeps running the client it
            # loaded with -- silently, and for as long as the tab is open
            # (measured: the hook object is identical after a tick, and a marker
            # set in the page survives). The running client compares the id it
            # mounted with against the one arriving; a difference means the tab
            # is old, and it says so instead of quietly refusing every drag.
            "client": CLIENT_ID,
            "client_css": CLIENT_CSS,
            "client_js": CLIENT_JS,
        },
        "children": children,
    }


def main():
    doc = json.load(sys.stdin)
    body = doc.get("body") or {}
    envelope = doc.get("envelope") or {}

    graph = body.get("graph")
    if not isinstance(graph, dict) or not isinstance(graph.get("nodes"), list):
        return []
    if not graph["nodes"]:
        # Nothing to draw. Silence rather than an empty picture: the timer will
        # bring the next snapshot, and a view that blanked itself because one
        # message was malformed is worse than one that is a minute stale.
        return []

    # `target` is this cell's own path, and the display will read the same string
    # off the emission's `reply_to`. It is the owner half of the view's identity,
    # and the display refuses a body that claims a different one (`not_owner`) --
    # which is why nothing here puts an `owner` in the body at all.
    return [{
        "header": {"route": "view"},
        "messages": [],
        "view_id": VIEW_ID,
        "region": REGION,
        "kind": "component",
        "content": content(graph, str(envelope.get("target") or "")),
        "components": components(),
    }]


if __name__ == "__main__":
    out = main()
    # A single emission is written as an object, several as an array -- and an
    # empty list stays an empty array, which is how a `code` cell says "nothing to
    # send" (`parse_stdout_json`: a top-level array of length 0 is zero emissions).
    sys.stdout.write(json.dumps(out[0] if len(out) == 1 else out))
