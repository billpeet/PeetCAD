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
      const items = drawItems(segs, ps, plane, edges, [0, 0, 0], counts);
      const drawn = apply(file, { op: "draw", sketch: sketchName, draw: items });
      if (!drawn.ok) { row.left.push(`${libName} ${f.name}: ${drawn.error}`); continue; }
      const smartName = features.filter(x => x.type !== "sketch" && x.type !== "linear_pattern").length > 1 ? `${libName} ${f.name}` : libName;
      // SolidWorks' cut goes into the face its sketch is on, against the sketch's normal; PeetCAD's smart
      // feature cuts into the face it is applied to, so only a reversed cut is reversed
      const reverse = !!(f.end_condition && f.end_condition.reverse_direction);
      const made = apply(file, { op: "smart_feature", name: smartName, sketch: sketchName, reference: lf.index, ...end, ...(reverse ? { reverse: true } : {}) });
      if (!made.ok) { row.left.push(`${libName} ${f.name}: ${made.error}`); continue; }
      drawnBy.set(f.name, { segs, ps, plane, sketch: sketchName, smart: smartName });
      row.smart_features.push({ feature: smartName, from: f.name, reference: lf.index, ...end, reverse, drawn: counts.drawn, converted: counts.converted });
    }
  }
  console.log(`${String(row.smart_features.length).padStart(3)} smart features  ${stem}${row.left.length ? "  (left out: " + row.left.join("; ") + ")" : ""}`);
}
fs.writeFileSync(path.join(library, "smart-report.json"), JSON.stringify(report, null, 1));
