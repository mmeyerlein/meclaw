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
      const d = G.edgePath(a, b, NODE_W, NODE_H, lane);
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

  /// The hive frames INSIDE a hive, so the nested rectangles travel with it too.
  /// Ancestors are deliberately left alone: their frames are derived and the
  /// server's answer grows them, which is the honest provisional picture — a
  /// parent that stretched on the client would be guessing.
  function nestedFrames(el, hive) {
    const prefix = hive + "/";
    return Array.from(el.querySelectorAll("[data-hive]")).filter(function (g) {
      return (g.getAttribute("data-hive") || "").startsWith(prefix);
    });
  }

  /// A hive group's `<rect>` and `<text>`, so a drag can move the frame with its
  /// contents instead of leaving it behind for one round trip.
  function hiveParts(g) {
    return {rect: g.querySelector ? g.querySelector("rect") : null,
            text: g.querySelector ? g.querySelector("text") : null};
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
      this.wire();
      applyCamera(this.el, this.cam);
      drawEdges(this.el);
    },
    // The server re-renders the whole tree, so its `transform` and its camera
    // attributes land again with every diff. Re-applying is not a workaround for
    // the SSR model, it IS the model: the client owns the view, the server owns
    // the picture.
    updated() {
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
        } else if (t.closest && t.closest("#detail")) {
          return;                       // the panel handles its own clicks
        } else {
          hook.sel = null;
        }
        if (hook.sel) select(el, hook.sel); else clearSelection(el);
      };

      this.onDown = function (ev) {
        const g = ev.target.closest("[data-node]");
        if (!g) {
          // Not a cell. A hive is the next thing worth grabbing: its frame and
          // its label are the group's handle, and moving a group is the gesture
          // that makes a 50-cell picture arrangeable at all — one drag instead of
          // twenty.
          const hg = ev.target.closest ? ev.target.closest("[data-hive]") : null;
          if (hg) {
            const id = hg.getAttribute("data-hive");
            const parts = hiveParts(hg);
            const members = membersOf(el, id);
            const ids = members.map(m => m.getAttribute("data-node"));
            // Own frame first, then every frame inside it — each with the
            // coordinates it started from, so a move is one addition and not an
            // accumulation that drifts.
            const frames = [hg].concat(nestedFrames(el, id)).map(function (g) {
              const p = hiveParts(g);
              return {
                rect: p.rect, text: p.text,
                rx: numAttr(p.rect, "x"), ry: numAttr(p.rect, "y"),
                tx: numAttr(p.text, "x"), ty: numAttr(p.text, "y"),
              };
            });
            hive = {
              id: id,
              g: hg,
              parts: parts,
              frames: frames,
              origin: {x: numAttr(parts.rect, "x"), y: numAttr(parts.rect, "y")},
              members: members.map(m => ({g: m, at: boxOf(m)})),
              from: userPoint(el, ev),
              delta: {x: 0, y: 0},
              // Every edge with an end inside this hive moves with it.
              edges: Array.from(el.querySelectorAll("path.edge")).filter(function (p) {
                return ids.indexOf(p.getAttribute("data-from")) >= 0 ||
                       ids.indexOf(p.getAttribute("data-to")) >= 0;
              }),
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
        const attached = Array.from(el.querySelectorAll(
          '[data-from="' + id + '"], [data-to="' + id + '"]'));
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
            hive.frames.forEach(function (f) {
              if (f.rect) {
                f.rect.setAttribute("x", f.rx + dx);
                f.rect.setAttribute("y", f.ry + dy);
              }
              if (f.text) {
                f.text.setAttribute("x", f.tx + dx);
                f.text.setAttribute("y", f.ty + dy);
              }
            });
            const boxes = {};
            hive.members.forEach(function (m) {
              m.g.setAttribute("transform",
                "translate(" + (m.at.x + dx) + "," + (m.at.y + dy) + ")");
            });
            el.querySelectorAll("[data-node]").forEach(function (g) {
              boxes[g.getAttribute("data-node")] = boxOf(g);
            });
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

      el.addEventListener("pointerdown", this.onDown);
      el.addEventListener("pointermove", this.onMove);
      el.addEventListener("pointerup", this.onUp);
      el.addEventListener("pointercancel", this.onUp);
      el.addEventListener("wheel", this.onWheel, {passive: false});
      el.addEventListener("click", this.onClick);
    },

    unwire() {
      this.el.removeEventListener("pointerdown", this.onDown);
      this.el.removeEventListener("pointermove", this.onMove);
      this.el.removeEventListener("pointerup", this.onUp);
      this.el.removeEventListener("pointercancel", this.onUp);
      this.el.removeEventListener("wheel", this.onWheel);
      this.el.removeEventListener("click", this.onClick);
    },
  };

  // The one name the binary and this file agree on: the dead render offers the
  // slot, the surface fills it.
  root.SurfaceHooks = Object.assign(root.SurfaceHooks || {}, {Canvy: Canvy});
})(typeof window !== "undefined" ? window : globalThis);
