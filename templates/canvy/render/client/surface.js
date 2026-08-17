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

  /// Which side of the box faces the target, as a unit vector.
  ///
  /// Decided by comparing the gap on each axis against the box's own aspect
  /// ratio, so a wide flat box prefers its top and bottom rather than its
  /// narrow sides whenever the target is even slightly above or below.
  function side(a, b, w, h) {
    const dx = (b.x + w / 2) - (a.x + w / 2);
    const dy = (b.y + h / 2) - (a.y + h / 2);
    if (Math.abs(dx) * h > Math.abs(dy) * w) {
      return dx >= 0 ? {x: 1, y: 0} : {x: -1, y: 0};
    }
    return dy >= 0 ? {x: 0, y: 1} : {x: 0, y: -1};
  }

  /// The point where a side's outward normal leaves the box.
  function anchor(box, dir, w, h, lane) {
    const cx = box.x + w / 2, cy = box.y + h / 2;
    // The lane offset slides the anchor ALONG the chosen side, so two edges
    // between the same pair of cells do not overlap into one line.
    const along = dir.x === 0 ? {x: 1, y: 0} : {x: 0, y: 1};
    const limit = dir.x === 0 ? w / 2 - 12 : h / 2 - 8;
    const off = Math.max(-limit, Math.min(limit, lane * LANE));
    return {
      x: cx + dir.x * (w / 2) + along.x * off,
      y: cy + dir.y * (h / 2) + along.y * off,
    };
  }

  /// An orthogonal path from box `a` to box `b`, as an SVG path string.
  ///
  /// `lane` separates parallel edges; pass the index among edges sharing this
  /// pair, centred on zero.
  function route(a, b, w, h, lane) {
    lane = lane || 0;
    const da = side(a, b, w, h);
    const db = {x: -da.x, y: -da.y};
    const p0 = anchor(a, da, w, h, lane);
    const p3 = anchor(b, db, w, h, lane);
    const p1 = {x: p0.x + da.x * STUB, y: p0.y + da.y * STUB};
    const p2 = {x: p3.x + db.x * STUB, y: p3.y + db.y * STUB};

    // Between the two stubs, turn at most twice. Horizontal exit means travel
    // horizontally first; vertical exit means vertically first.
    const mid = da.x !== 0
      ? [{x: p1.x, y: p1.y}, {x: (p1.x + p2.x) / 2, y: p1.y},
         {x: (p1.x + p2.x) / 2, y: p2.y}, {x: p2.x, y: p2.y}]
      : [{x: p1.x, y: p1.y}, {x: p1.x, y: (p1.y + p2.y) / 2},
         {x: p2.x, y: (p1.y + p2.y) / 2}, {x: p2.x, y: p2.y}];

    const pts = [p0, ...mid, p3];
    return {d: rounded(pts), start: p0, end: p3};
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
  function edgePath(a, b, w, h, lane) {
    return route(a, b, w, h, lane).d;
  }

  const api = {STUB, LANE, side, anchor, route, rounded, edgePath, segmentHitsBox};
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
  function drawEdges(el) {
    const nodes = {};
    el.querySelectorAll("[data-node]").forEach(function (g) {
      nodes[g.getAttribute("data-node")] = boxOf(g);
    });
    el.querySelectorAll("path.edge").forEach(function (p) {
      const a = nodes[p.getAttribute("data-from")];
      const b = nodes[p.getAttribute("data-to")];
      if (!a || !b) { p.removeAttribute("d"); return; }
      const lane = parseInt(p.getAttribute("data-lane") || "0", 10);
      p.setAttribute("d", G.edgePath(a, b, NODE_W, NODE_H, lane));
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

  const Canvy = {
    mounted() {
      this.cam = cameraOf(this.el);
      this.wire();
      applyCamera(this.el, this.cam);
      drawEdges(this.el);
    },
    // The server re-renders the whole tree, so its `transform` and its camera
    // attributes land again with every diff. Re-applying is not a workaround for
    // the SSR model, it IS the model: the client owns the view, the server owns
    // the picture.
    updated() { applyCamera(this.el, this.cam); drawEdges(this.el); },
    destroyed() { this.unwire(); },

    wire() {
      const el = this.el, hook = this;
      let drag = null;
      let pan = null;
      let frame = null;

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
      };

      this.onDown = function (ev) {
        const g = ev.target.closest("[data-node]");
        if (!g) {
          // Empty canvas: pan. A picture larger than its frame with no way to
          // move is the same defect as no picture at all.
          pan = {from: {x: ev.clientX, y: ev.clientY},
                 origin: {x: hook.cam.x, y: hook.cam.y}};
          el.classList.add("panning");
          return;
        }
        const id = g.getAttribute("data-node");
        // Collect the attached edges ONCE, not per frame.
        const attached = Array.from(el.querySelectorAll(
          '[data-from="' + id + '"], [data-to="' + id + '"]'));
        drag = {id: id, g: g, origin: boxOf(g), from: userPoint(el, ev),
                at: boxOf(g), edges: attached};
        g.setPointerCapture && g.setPointerCapture(ev.pointerId);
        ev.preventDefault();
      };

      this.onMove = function (ev) {
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
          const boxes = {};
          el.querySelectorAll("[data-node]").forEach(function (g) {
            boxes[g.getAttribute("data-node")] = boxOf(g);
          });
          drag.edges.forEach(function (p) {
            const a = boxes[p.getAttribute("data-from")];
            const b = boxes[p.getAttribute("data-to")];
            if (a && b) rubberBand(p, a, b);
          });
        });
      };

      this.onUp = function (ev) {
        if (pan) {
          pan = null;
          el.classList.remove("panning");
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
      };

      el.addEventListener("pointerdown", this.onDown);
      el.addEventListener("pointermove", this.onMove);
      el.addEventListener("pointerup", this.onUp);
      el.addEventListener("pointercancel", this.onUp);
      el.addEventListener("wheel", this.onWheel, {passive: false});
    },

    unwire() {
      this.el.removeEventListener("pointerdown", this.onDown);
      this.el.removeEventListener("pointermove", this.onMove);
      this.el.removeEventListener("pointerup", this.onUp);
      this.el.removeEventListener("pointercancel", this.onUp);
      this.el.removeEventListener("wheel", this.onWheel);
    },
  };

  // The one name the binary and this file agree on: the dead render offers the
  // slot, the surface fills it.
  root.SurfaceHooks = Object.assign(root.SurfaceHooks || {}, {Canvy: Canvy});
})(typeof window !== "undefined" ? window : globalThis);
