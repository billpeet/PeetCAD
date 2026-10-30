#!/usr/bin/env node
/**
 * Gives the parts of a PeetCAD template library the smart features their SolidWorks templates carry
 * (SolidWorks' smart components: a hinge's holes, a vent hood's cut-out, made in the part they are placed
 * against by FabSys's ApplySmartFeatures), from their definitions read out of the SolidWorks files:
 *
 *   node tools/sw-import/smart.js <smart-defs.json> <library folder> [--peet <peet>] [--only <text>] [--force]
 *
 * The SolidWorks export has no definition of a smart component (its SmartComponentFeature is empty), so the
 * definitions come from the template files themselves: each holds its library features as embedded documents,
 * which a separate decoder reads into smart-defs.json (the sketch of each cut, hole and pattern, in the smart
 * part's coordinates, and its end condition). For each library feature of a template, each of its cuts and
 * holes becomes a sketch of the part on a plane where SolidWorks' training target's face was, and a
 * `smart_feature` of it, reference number = the library feature's (the order FabSys gives the faces in). A
 * pattern's copies are drawn in its seed's sketch. A line, arc or circle that SolidWorks converted from the
 * part's own edges is projected from the matching edge (`convert`), so it follows the part when it is resized;
 * the rest is drawn where SolidWorks had it. Writes smart-report.json in the library folder.
 */
const fs = require("fs");
const path = require("path");
const { spawnSync } = require("child_process");

const args = process.argv.slice(2);
const option = name => { const i = args.indexOf(name); if (i < 0) return null; const v = args[i + 1]; args.splice(i, 2); return v; };
const flag = name => { const i = args.indexOf(name); if (i < 0) return false; args.splice(i, 1); return true; };
const repo = path.resolve(__dirname, "..", "..");
const peetExe = option("--peet")
  || [path.join(repo, "target", "release", "peet.exe"), path.join(repo, "target", "debug", "peet.exe"), "peet"].find(p => p === "peet" || fs.existsSync(p));
const only = option("--only");
const force = flag("--force");
if (args.length !== 2) {
  console.error("Usage: node smart.js <smart-defs.json> <library folder> [--peet <peet>] [--only <text>] [--force]");
  process.exit(2);
}
const [defsFile, library] = args.map(a => path.resolve(a));
const MM = 1000;
const tidy = v => Math.abs(v) < 1e-9 ? 0 : +v.toFixed(6);
const dot = (a, b) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
const sub = (a, b) => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
const add = (a, b) => [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
const mul = (a, k) => a.map(x => x * k);
const len = a => Math.hypot(...a);
const mm = p => p.map(x => x * MM);

function run(file, ops, save) {
  const r = spawnSync(peetExe, ["run", "-", "-f", file, "-q", ...(save ? [] : ["--no-save"])],
    { input: ops.map(o => JSON.stringify(o)).join("\n") + "\n", encoding: "utf8", maxBuffer: 1 << 28 });
  return (r.stdout || "").split("\n").filter(l => l.startsWith("{")).map(l => JSON.parse(l));
}
const ask = (file, op) => run(file, [op], false)[0] || { ok: false, error: "no reply" };
const apply = (file, op) => run(file, [op], true)[0] || { ok: false, error: "no reply" };

const END = { up_to_next: { end: "sheet" }, through_all: { end: "through_all" } };

/** A standard plane PeetCAD has whose normal is along `n`, and the offset along its normal to `o`. */
function planeOp(o, n, name) {
  const axes = [["right", [1, 0, 0]], ["front", [0, -1, 0]], ["top", [0, 0, 1]]];
  for (const [from, d] of axes) {
    if (Math.abs(Math.abs(dot(n, d)) - 1) > 1e-6) continue;
    const distance = dot(o, d);
    return { op: "plane", from, distance: tidy(Math.abs(distance)), flip: distance < 0, name };
  }
  return null;
}

/** The part's edge a decoded segment was converted from, if one projects onto it exactly. */
function convertedEdge(edges, seg, plane) {
  const flat = p => sub(p, mul(plane.n, dot(sub(p, plane.o), plane.n)));
  const near = (a, b) => len(sub(flat(a), flat(b))) < 0.01;
  for (const e of edges) {
    if (Math.abs(dot(e.curve === "line" ? e.direction || [0, 0, 0] : [0, 0, 0], plane.n)) > 1e-6) continue;
    if (seg.kind === "line" && e.curve === "line") {
      const [a, b] = [mm(seg.start_3d.smart), mm(seg.end_3d.smart)];
      if ((near(a, e.from) && near(b, e.to)) || (near(a, e.to) && near(b, e.from))) return e;
    }
    if ((seg.kind === "circle" || seg.kind === "arc") && e.curve === "circle" && e.axis && Math.abs(Math.abs(dot(e.axis, plane.n)) - 1) < 1e-6) {
      if (!near(mm(seg.centre_3d.smart), e.center) || Math.abs(seg.radius * MM - e.radius) > 0.01) continue;
      const whole = len(sub(e.from, e.to)) < 1e-6;
      if (seg.kind === "circle" ? whole : !whole && ((near(mm(seg.start_3d.smart), e.from) && near(mm(seg.end_3d.smart), e.to)) || (near(mm(seg.start_3d.smart), e.to) && near(mm(seg.end_3d.smart), e.from)))) return e;
    }
  }
  return null;
}

/**
 * Draw items that make a decoded profile follow the part, as SolidWorks' does: the part's edges SolidWorks
 * converted as construction lines are projected from the part, each line of the profile is held its distance
 * from the projected line it runs alongside, its ends joined and its arcs' radii held. Null if nothing is
 * converted.
 */
function followingItems(segs, ps, plane, edges, counts) {
  const flat = p => { const d = sub(p, ps.o); return [dot(d, ps.x), dot(d, ps.y)]; };
  const items = [], lines = [], drawn = [];
  // The middle of the profile: the converted edges are its part's outline round it
  const live = segs.filter(g => !g.deleted && !g.construction && g.start_3d);
  if (!live.length) return null;
  const mid = flat(mul(live.reduce((s, g) => add(s, mm(g.start_3d.smart)), [0, 0, 0]), 1 / live.length));
  // The part's edge each converted line was converted from: one in line with it, where the part is the size it
  // was when SolidWorks' smart feature was made. Where it is not (Gland Plate: 400 x 200 now, 500 x 400 then)
  // and the converted lines are the part's outline round the profile, the outermost edge alongside each on its
  // side. Otherwise the profile is drawn where SolidWorks had it.
  const found = [];
  segs.forEach((g, i) => {
    if (g.deleted || !g.construction || !g.external_reference || g.kind !== "line") return;
    const a = flat(mm(g.start_3d.smart)), b = flat(mm(g.end_3d.smart));
    const u = [b[0] - a[0], b[1] - a[1]], ul = Math.hypot(...u);
    if (ul < 1e-9) return;
    // Outward: from the profile's middle across to the converted edge
    let out = [-u[1] / ul, u[0] / ul];
    if ((a[0] - mid[0]) * out[0] + (a[1] - mid[1]) * out[1] < 0) out = [-out[0], -out[1]];
    const at = (a[0] - mid[0]) * out[0] + (a[1] - mid[1]) * out[1];
    let inLine = null, outer = null, far = -Infinity;
    for (const x of edges) {
      if (x.curve !== "line" || Math.abs(dot(x.direction || [0, 0, 1], plane.n)) > 1e-6) continue;
      const p = flat(x.from), q = flat(x.to), v = [q[0] - p[0], q[1] - p[1]];
      if (Math.abs(u[0] * v[1] - u[1] * v[0]) > 1e-6 * ul * Math.hypot(...v)) continue;
      const dist = (p[0] - mid[0]) * out[0] + (p[1] - mid[1]) * out[1];
      if (Math.abs(dist - at) < 1e-3 && !inLine) inLine = x;
      if (dist > far + 1e-6) { far = dist; outer = x; }
    }
    found.push({ i, a, b, out, at, inLine, outer, far });
  });
  if (!found.length) return null;
  let use;
  if (found.every(f => f.inLine)) use = found.map(f => ({ ...f, e: f.inLine, delta: 0 }));
  else {
    // An outline round the profile: converted lines facing every way out of it
    const ways = new Set(found.map(f => `${Math.round(f.out[0])},${Math.round(f.out[1])}`));
    const round = ["1,0", "-1,0", "0,1", "0,-1"].every(w => ways.has(w)) && found.every(f => f.outer && f.far > 0);
    if (!round) return null;
    use = found.map(f => ({ ...f, e: f.outer, delta: f.far - f.at }));
  }
  for (const f of use) {
    items.push({ type: "project", edge: { at: f.e.middle }, as: `p${f.i}` });
    // Where it was when SolidWorks made the smart feature: the profile keeps its distances from that
    lines.push({ label: `p${f.i}`, a: f.a, b: f.b, out: f.out, delta: f.delta });
  }
  if (!lines.length) return null;
  // Drawn where it comes to on the part as it is now (each point moved out as far as the edges on its side
  // moved), so that the distances held keep it on the inside, as it is drawn
  const moved = p => {
    let q = [p[0], p[1]];
    for (const l of lines) if ((p[0] - mid[0]) * l.out[0] + (p[1] - mid[1]) * l.out[1] > 0) q = [q[0] + l.delta * l.out[0], q[1] + l.delta * l.out[1]];
    return q.map(tidy);
  };
  segs.forEach((g, i) => {
    if (g.deleted || g.construction) return;
    const label = `s${i}`;
    if (g.kind === "line") {
      const a = flat(mm(g.start_3d.smart)), b = flat(mm(g.end_3d.smart));
      items.push({ type: "line", from: moved(a), to: moved(b), as: label });
      drawn.push({ label, kind: "line", start: a, end: b });
    } else if (g.kind === "arc") {
      let [s, t] = [mm(g.start_3d.smart), mm(g.end_3d.smart)];
      if (g.axis_ccw && dot(g.axis_ccw.smart, ps.n) < 0) [s, t] = [t, s];
      items.push({ type: "arc", center: moved(flat(mm(g.centre_3d.smart))), start: moved(flat(s)), end: moved(flat(t)), as: label });
      items.push({ type: "radius", of: [label], value: tidy(g.radius * MM) });
      drawn.push({ label, kind: "arc", start: flat(s), end: flat(t), center: flat(mm(g.centre_3d.smart)) });
    } else if (g.kind === "circle") {
      items.push({ type: "circle", center: moved(flat(mm(g.centre_3d.smart))), radius: tidy(g.radius * MM), as: label });
    } else return;
    counts.drawn++;
  });
  const d2 = (p, q) => Math.hypot(p[0] - q[0], p[1] - q[1]);
  // Ends that meet are joined; a line meeting an arc square to its radius is tangent to it
  for (let i = 0; i < drawn.length; i++) for (let j = i + 1; j < drawn.length; j++) {
    for (const [ka, pa] of [["start", drawn[i].start], ["end", drawn[i].end]]) for (const [kb, pb] of [["start", drawn[j].start], ["end", drawn[j].end]]) {
      if (d2(pa, pb) > 1e-4) continue;
      items.push({ type: "coincident", of: [`${drawn[i].label}.${ka}`, `${drawn[j].label}.${kb}`] });
      const [l, a] = drawn[i].kind === "line" && drawn[j].kind === "arc" ? [drawn[i], drawn[j]] : drawn[j].kind === "line" && drawn[i].kind === "arc" ? [drawn[j], drawn[i]] : [null, null];
      if (l) {
        const dir = [l.end[0] - l.start[0], l.end[1] - l.start[1]], r = [pa[0] - a.center[0], pa[1] - a.center[1]];
        if (Math.abs(dir[0] * r[0] + dir[1] * r[1]) < 1e-6 * Math.hypot(...dir) * Math.hypot(...r) + 1e-9) items.push({ type: "tangent", of: [l.label, a.label] });
      }
    }
  }
  // Each line its distance from the projected line it runs alongside (the nearest parallel one)
  for (const l of drawn.filter(x => x.kind === "line")) {
    const u = [l.end[0] - l.start[0], l.end[1] - l.start[1]], n = Math.hypot(...u);
    let best = null;
    for (const p of lines) {
      const v = [p.b[0] - p.a[0], p.b[1] - p.a[1]], m = Math.hypot(...v);
      if (Math.abs(u[0] * v[1] - u[1] * v[0]) > 1e-6 * n * m) continue;
      const off = Math.abs((l.start[0] - p.a[0]) * v[1] - (l.start[1] - p.a[1]) * v[0]) / m;
      if (!best || off < best.off) best = { off, p };
    }
    if (best) items.push(best.off > 1e-6 ? { type: "offset_distance", of: [l.label, best.p.label], value: tidy(best.off) } : { type: "collinear", of: [l.label, best.p.label] });
  }
  counts.converted += lines.length;
  // How far the part's edges are from where they were when SolidWorks made it: the profile moved as much
  counts.moved = Math.max(counts.moved || 0, ...lines.map(l => Math.abs(l.delta)));
  return items;
}

/** Draw items for decoded segments, moved by `shift` (mm), in the PeetCAD sketch's plane `ps`. */
function drawItems(segs, ps, plane, edges, shift, counts) {
  const flat = p => { const d = sub(add(p, shift), ps.o); return [tidy(dot(d, ps.x)), tidy(dot(d, ps.y))]; };
  const items = [];
  for (const g of segs) {
    if (g.deleted || g.construction) continue;
    if (g.external_reference && len(shift) < 1e-9) {
      const e = convertedEdge(edges, g, plane);
      if (e) { items.push({ type: "project", edge: { at: e.middle }, convert: true }); counts.converted++; continue; }
    }
    if (g.kind === "line") items.push({ type: "line", from: flat(mm(g.start_3d.smart)), to: flat(mm(g.end_3d.smart)) });
    else if (g.kind === "circle") items.push({ type: "circle", center: flat(mm(g.centre_3d.smart)), radius: tidy(g.radius * MM) });
    else if (g.kind === "arc") {
      let [s, t] = [mm(g.start_3d.smart), mm(g.end_3d.smart)];
      if (g.axis_ccw && dot(g.axis_ccw.smart, ps.n) < 0) [s, t] = [t, s];
      items.push({ type: "arc", center: flat(mm(g.centre_3d.smart)), start: flat(s), end: flat(t) });
    } else continue;
    counts.drawn++;
  }
  return items;
}

const defs = JSON.parse(fs.readFileSync(defsFile, "utf8"));
const report = [];
for (const t of defs.templates) {
  if (!t.library_features || !t.library_features.length) continue;
  const stem = t.template.replace(/\.(sldprt|sldasm)$/i, "");
  if (only && !stem.toLowerCase().includes(only.toLowerCase())) continue;
  const file = path.join(library, stem + ".peet");
  const row = { template: stem, smart_features: [], left: [] };
  report.push(row);
  if (!fs.existsSync(file)) { row.left.push("not in the library"); console.log(`not in the library   ${stem}`); continue; }
  const listed = ask(file, { op: "features" });
  if ((listed.features || []).some(f => f.type === "Smart feature") && !force) { row.left.push("has smart features already"); continue; }
  const edges = ask(file, { op: "edges" }).edges || [];
  for (const lf of t.library_features) {
    const libName = (lf.lf_name || `Library feature ${lf.index}`).replace(/_lf$/i, "");
    const features = lf.features || [];
    // What each cut or hole draws, so a pattern after it can copy it
    const drawnBy = new Map();
    let planeMade = null;
    for (const f of features) {
      const counts = { drawn: 0, converted: 0 };
      let segs = null, sk = null, end = null;
      if (f.type === "cut_extrude" && f.sketch) {
        sk = f.sketch; segs = sk.segments || [];
        end = END[f.end_condition && f.end_condition.type];
        if (!end && f.end_condition && f.end_condition.type === "blind") end = { end: "blind", depth: tidy(f.end_condition.depth * MM) };
      } else if (f.type === "hole_wizard" && f.hole && f.sketches && f.sketches.length) {
        sk = f.sketches[0];
        const r = (f.hole.parameters && (f.hole.parameters["Thru Hole Dia."] || f.hole.parameters["Hole Dia."])) || f.hole.nominal_diameter_from_name;
        segs = (f.hole.centres || []).map(c => ({ kind: "circle", centre_3d: { smart: c.smart }, radius: r / 2 }));
        end = END[f.end_condition && f.end_condition.type] || (f.end_condition && f.end_condition.type === "blind" ? { end: "blind", depth: tidy(f.end_condition.depth_or_thickness_value * MM) } : null);
      } else if (f.type === "linear_pattern" && f.pattern) {
        const dir = f.pattern.direction1_inferred && f.pattern.direction1_inferred.smart;
        const g = f.pattern.direction1_guess || {};
        const seed = [...drawnBy.values()].pop();
        if (!dir || !g.count || !g.spacing || !seed) { row.left.push(`${libName} ${f.name}: the pattern's direction or seed is not known`); continue; }
        const items = [];
        for (let k = 1; k < Math.round(g.count); k++) items.push(...drawItems(seed.segs, seed.ps, seed.plane, edges, mul(dir, k * g.spacing * MM), counts));
        const r = apply(file, { op: "draw", sketch: seed.sketch, draw: items });
        if (!r.ok) row.left.push(`${libName} ${f.name}: ${r.error}`);
        else row.smart_features.push({ feature: seed.smart, pattern: f.name, copies: Math.round(g.count) - 1, drawn: counts.drawn });
        continue;
      } else continue;
      if (!end) { row.left.push(`${libName} ${f.name}: end condition ${JSON.stringify(f.end_condition)}`); continue; }
      const o = mm(sk.plane.origin.smart), n = sk.plane.normal.smart;
      const plane = { o, n };
      const planeName = `${libName} Plane`;
      if (!planeMade) {
        const op = planeOp(o, n, planeName);
        if (!op) { row.left.push(`${libName} ${f.name}: its plane is not square to an axis`); continue; }
        const made = apply(file, op);
        if (!made.ok) { row.left.push(`${libName}: ${made.error}`); continue; }
        planeMade = { o, n };
      } else if (Math.abs(Math.abs(dot(planeMade.n, n)) - 1) > 1e-6 || Math.abs(dot(sub(o, planeMade.o), n)) > 1e-3) {
        row.left.push(`${libName} ${f.name}: its sketch is on another plane than the library feature's first`); continue;
      }
      const sketchName = `${libName} ${sk.name || f.name + " sketch"}`;
      const started = apply(file, { op: "sketch", on: planeName, name: sketchName, draw: [] });
      if (!started.ok || !started.plane) { row.left.push(`${libName} ${f.name}: ${started.error}`); continue; }
      const ps = { o: started.plane.origin, x: started.plane.x, y: started.plane.y, n: started.plane.normal };
      // Following the part where SolidWorks' does, else drawn where SolidWorks had it
      const following = followingItems(segs, ps, plane, edges, counts);
      let drawn = following ? apply(file, { op: "draw", sketch: sketchName, draw: following }) : null;
      if (drawn && !drawn.ok) row.left.push(`${libName} ${f.name}: drawn fixed, as it can't follow the part (${drawn.error})`);
      if (!drawn || !drawn.ok) {
        counts.drawn = 0; counts.converted = 0;
        drawn = apply(file, { op: "draw", sketch: sketchName, draw: drawItems(segs, ps, plane, edges, [0, 0, 0], counts) });
      }
      if (!drawn.ok) { row.left.push(`${libName} ${f.name}: ${drawn.error}`); continue; }
      const smartName = features.filter(x => x.type !== "sketch" && x.type !== "linear_pattern").length > 1 ? `${libName} ${f.name}` : libName;
      // SolidWorks' cut goes into the face its sketch is on, against the sketch's normal; PeetCAD's smart
      // feature cuts into the face it is applied to, so only a reversed cut is reversed
      const reverse = !!(f.end_condition && f.end_condition.reverse_direction);
      const made = apply(file, { op: "smart_feature", name: smartName, sketch: sketchName, reference: lf.index, ...end, ...(reverse ? { reverse: true } : {}) });
      if (!made.ok) { row.left.push(`${libName} ${f.name}: ${made.error}`); continue; }
      drawnBy.set(f.name, { segs, ps, plane, sketch: sketchName, smart: smartName });
      row.smart_features.push({ feature: smartName, from: f.name, reference: lf.index, ...end, reverse, drawn: counts.drawn, converted: counts.converted, ...(counts.moved ? { moved_mm: tidy(counts.moved) } : {}) });
    }
  }
  console.log(`${String(row.smart_features.length).padStart(3)} smart features  ${stem}${row.left.length ? "  (left out: " + row.left.join("; ") + ")" : ""}`);
}
fs.writeFileSync(path.join(library, "smart-report.json"), JSON.stringify(report, null, 1));
