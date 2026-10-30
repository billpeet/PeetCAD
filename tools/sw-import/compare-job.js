#!/usr/bin/env node
/**
 * Compares what FabSys built for a job in PeetCAD with what it built in SolidWorks, or with what a dry run of the
 * job says it should build.
 *
 *   node tools/sw-import/compare-job.js <SolidWorks side> <PeetCAD side> [--out report.json] [--peet <peet>]
 *        [--sw-root <name>] [--peet-root <name>] [--sw-job <job number>] [--peet-job <job number>]
 *        [--templates-exports <folder>] [--edited <list.txt>] [--tolerance <mm>] [--no-parts]
 *
 * The SolidWorks side is one of:
 * - a folder of exports (tools/sw-export, best with --step and --dxf) of the job's root assembly and its parts and
 *   sub-assemblies: the root is the assembly export whose name has "Enclosure Assembly" in it (or --sw-root);
 * - a FabSys.Run report of a dry run of the job (`--backend dryrun --report x.json`, FabSys.Run's report has the dry
 *   run's tree): the tree the script asks for, in SolidWorks' names, with no geometry. Template assemblies FabSys
 *   copies are filled in from their exports (--templates-exports, the library's: peetcad-imports\exports\v8-asm).
 *
 * The PeetCAD side is FabSys.Run's --out folder of a PeetCAD run: the root assembly (the .peet whose name has
 * "Enclosure Assembly" in it, or --peet-root) and the parts and sub-assemblies beside it.
 *
 * Components are matched by their path of instance names, with each side's job number taken out of every name
 * (FabSys names files "<JobNumber> <Prefix> <Name>", so one job run twice under two numbers still matches): the job
 * number is the first word most file names share, or --sw-job / --peet-job. Instances that match only by their file
 * (numbered differently) are paired by where they are.
 *
 * For each component: whether it is on both sides, how far apart its placements are (mm, the largest distance
 * between where the two put points 100 mm round its origin, as import-asm.js measures), its configuration,
 * suppression and visibility, PeetCAD's status for it, and its part: volume, box and flat size (SolidWorks' STEP of
 * the configuration measured by PeetCAD where the export has one, the flat size from its flat DXF or flat-pattern
 * configuration), which features are suppressed, and the dimensions both have by the same address. Mates of each
 * assembly both sides have, by name (FabSys names them "<type>_<component>_<entity>_..._"): present on both, and
 * whether each solves. The whole assembly's volume and box against SolidWorks' STEP of it (measured by PeetCAD), or
 * its mass properties. A verdict for each component and for the job.
 *
 * --edited takes a list of file names (one per line) changed by hand after the FabSys run in SolidWorks: the job
 * folders on the NAS are as the designers left them, not as FabSys did. Components of those files are marked.
 */
const fs = require("fs");
const path = require("path");
const { spawnSync } = require("child_process");

const MM = 1000;
const tidy = v => Math.abs(v) < 1e-12 ? 0 : +v.toFixed(6);

// ---- Transforms (as import-asm.js) ----
//
// SolidWorks' transform (MathTransform.ArrayData) is a rotation R applied to row vectors, then a translation:
// world = p·R + t, metres. PeetCAD places a component at a point with an axis-angle rotation, millimetres.

const IDENTITY = { R: [[1, 0, 0], [0, 1, 0], [0, 0, 1]], t: [0, 0, 0] };
const matrixOf = t => ({ R: [[t[0], t[1], t[2]], [t[3], t[4], t[5]], [t[6], t[7], t[8]]], t: [t[9], t[10], t[11]] });
const toWorld = (m, p) => [0, 1, 2].map(j => p[0] * m.R[0][j] + p[1] * m.R[1][j] + p[2] * m.R[2][j] + m.t[j]);
/** Where PeetCAD says a component is (`at`, `rotate`), as a SolidWorks-style matrix. */
function matrixFromPeet(c) {
  const [x, y, z] = (c.at || [0, 0, 0]).map(v => v / MM);
  if (!c.rotate || !c.rotate.angle) return { R: [[1, 0, 0], [0, 1, 0], [0, 0, 1]], t: [x, y, z] };
  const a = c.rotate.angle * Math.PI / 180, [u, v, w] = c.rotate.axis;
  const C = Math.cos(a), S = Math.sin(a), T = 1 - C;
  const A = [[T * u * u + C, T * u * v - S * w, T * u * w + S * v], [T * u * v + S * w, T * v * v + C, T * v * w - S * u], [T * u * w - S * v, T * v * w + S * u, T * w * w + C]];
  return { R: [0, 1, 2].map(i => [0, 1, 2].map(j => A[j][i])), t: [x, y, z] };
}
/** `m` then `n`: the placement `m` gives, placed by `n`. */
const then = (m, n) => ({
  R: [0, 1, 2].map(i => [0, 1, 2].map(j => m.R[i][0] * n.R[0][j] + m.R[i][1] * n.R[1][j] + m.R[i][2] * n.R[2][j])),
  t: toWorld(n, m.t),
});
/** How far apart two placements are: the largest distance between where they put points 100 mm round the origin. */
function apart(a, b) {
  return Math.max(...[[0, 0, 0], [0.1, 0, 0], [0, 0.1, 0], [0, 0, 0.1]].map(p => {
    const [u, v] = [toWorld(a, p), toWorld(b, p)];
    return Math.hypot(u[0] - v[0], u[1] - v[1], u[2] - v[2]) * MM;
  }));
}

// ---- Names ----

/** A file's name without its folder and extension. */
const stem = f => path.basename(String(f || "").replace(/\\/g, "/")).replace(/\.(sldprt|sldasm|peet)$/i, "");
/** The instance number off an instance name: "Door-12" → ["Door", 12]. */
function splitInstance(name) {
  const m = /^(.*)-(\d+)$/.exec(name);
  return m ? [m[1], +m[2]] : [name, null];
}
/**
 * The job number of a side: the first word that more than half its files start with and that has a digit in it
 * (FabSys's "<JobNumber> ..."), or null.
 */
function jobOf(files) {
  const words = new Map();
  for (const f of files) {
    const w = String(f).split(" ")[0];
    if (/\d/.test(w) && String(f).includes(" ")) words.set(w, (words.get(w) || 0) + 1);
  }
  const best = [...words].sort((a, b) => b[1] - a[1])[0];
  return best && best[1] > files.length / 2 ? best[0] : null;
}
/** A name with the job number taken out, wherever a name starts with it. */
function withoutJob(name, job) {
  if (!job) return name;
  const escaped = job.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return String(name).replace(new RegExp(`(^|[/@_])${escaped}(?= )`, "g"), "$1<job>");
}
const keyOf = (segments, job) => segments.map(s => withoutJob(s, job).toLowerCase()).join("/");

// ---- peet ----

function peetRunner(peetExe) {
  const cache = new Map();
  return function ask(file, ops) {
    const key = JSON.stringify([file, ops]);
    if (cache.has(key)) return cache.get(key);
    const r = spawnSync(peetExe, ["op", ...ops.map(o => JSON.stringify(o)), ...(file ? ["-f", file] : []), "--no-save", "-q", "--keep-going"], { encoding: "utf8", maxBuffer: 1 << 29 });
    const replies = (r.stdout || "").split("\n").filter(l => l.startsWith("{")).map(l => { try { return JSON.parse(l); } catch { return { ok: false, error: "unreadable reply" }; } });
    if (!replies.length) replies.push({ ok: false, error: (r.stderr || "no reply from peet").trim() });
    cache.set(key, replies);
    return replies;
  };
}

/** A STEP file's solids as PeetCAD measures them: volume and box, or null. */
function measureStep(ask, file) {
  if (!file || !fs.existsSync(file)) return null;
  const [imported, mass] = ask(null, [{ op: "import_step", path: file }, { op: "mass" }]);
  if (!imported || !imported.ok || (imported.warnings || []).some(w => /left out/.test(w))) return null;
  const total = mass && mass.total;
  return total && total.volume_mm3 > 0 && total.min ? { volume: total.volume_mm3, min: total.min, max: total.max } : null;
}

/** The size of a DXF drawing, from its header (as import.js reads SolidWorks' flat patterns). */
function dxfSize(file) {
  if (!file || !fs.existsSync(file)) return null;
  const lines = fs.readFileSync(file, "latin1").split(/\r?\n/).map(l => l.trim());
  const at = name => { const i = lines.indexOf(name); return i < 0 ? null : [parseFloat(lines[i + 2]), parseFloat(lines[i + 4])]; };
  const min = at("$EXTMIN"), max = at("$EXTMAX");
  return min && max ? [tidy(max[0] - min[0]), tidy(max[1] - min[1])] : null;
}

// ---- The PeetCAD side ----

/**
 * A PeetCAD job: every component at every level (by path, placed in the root's frame), the mates of each assembly
 * document, and the root's mass. Sub-assemblies are read from their files in the configuration their component
 * shows; components that move in a flexible sub-assembly are where the parent's `moving` says.
 */
function readPeet(dir, ask, rootName, wanted = null) {
  const files = fs.readdirSync(dir).filter(f => /\.peet$/i.test(f));
  const rootFile = rootName ? files.find(f => stem(f).toLowerCase() === stem(rootName).toLowerCase())
    : files.find(f => /enclosure assembly/i.test(f));
  if (!rootFile) throw new Error(`no root assembly in ${dir}: give --peet-root`);
  const components = [];
  const assemblies = [];
  const walk = (file, configuration, parent, prefix, depth) => {
    const ops = [...(configuration ? [{ op: "configuration", configuration }] : []), { op: "components" }, { op: "mates" }];
    const replies = ask(file, ops);
    const [list, mates] = replies.slice(configuration ? 1 : 0);
    if (!list || !list.ok) { assemblies.push({ file: stem(file), path: prefix, configuration, error: (list && list.error) || "unreadable" }); return; }
    assemblies.push({ file: stem(file), path: prefix, configuration, mates: (mates && mates.mates) || [], freedom: mates && mates.freedom });
    const parts = new Map((list.parts || []).map(p => [p.name, p]));
    for (const c of list.components || []) {
      const part = parts.get(c.part) || {};
      const linkFile = part.link_file || (part.link ? path.join(path.dirname(file), part.link) : null);
      const row = {
        path: [...prefix, c.name], file: linkFile ? stem(linkFile) : c.part, linkFile, assembly: part.kind === "assembly",
        configuration: c.configuration || null, configurations: part.configurations || null,
        suppressed: c.status === "suppressed", hidden: !!c.hidden, flexible: !!c.flexible, fixed: !!c.fixed,
        matrix: then(matrixFromPeet(c), parent), status: c.status, message: c.message || null, freedom: c.freedom,
      };
      components.push(row);
      if (row.assembly && linkFile && !row.suppressed && depth < 8) {
        walk(linkFile, c.configuration || null, row.matrix, row.path, depth + 1);
        // Where its components are as it is solved here (in this document's frame)
        for (const x of c.moving || []) {
          const inner = components.find(r => r.path.join("/") === [...row.path, ...x.path].join("/"));
          if (inner) inner.matrix = then(matrixFromPeet(x), parent);
        }
      }
    }
  };
  const rootPath = path.join(dir, rootFile);
  // The root in the configuration SolidWorks' export has active, where it has one of that name
  const [configurations] = ask(rootPath, [{ op: "configurations" }]);
  const configuration = wanted && ((configurations && configurations.configurations) || []).some(c => c.name === wanted) ? wanted : null;
  walk(rootPath, configuration, IDENTITY, [], 0);
  const [mass] = ask(rootPath, configuration ? [{ op: "configuration", configuration }, { op: "mass" }] : [{ op: "mass" }]).slice(configuration ? 1 : 0);
  return { kind: "PeetCAD", dir, root: stem(rootFile), components, assemblies, mass: mass && mass.total ? mass.total : null };
}

/** A PeetCAD part in a configuration: its volume, box and flat size, its features' suppression, its dimensions. */
function peetPart(ask, file, configuration) {
  const replies = ask(file, [...(configuration ? [{ op: "configuration", configuration }] : []), { op: "bodies" }, { op: "features" }, { op: "dimensions" }]);
  const [bodies, features, dimensions] = replies.slice(configuration ? 1 : 0);
  if (!bodies || !bodies.ok) return { error: (bodies && bodies.error) || (replies[0] && replies[0].error) || "unreadable" };
  const list = bodies.bodies || [];
  const flat = list.find(b => b.flat_size);
  const failed = ((features && features.features) || []).filter(f => f.status === "failed" || f.status === "warning").map(f => f.name);
  return {
    volume: list.reduce((a, b) => a + (b.volume_mm3 || 0), 0),
    min: list.length ? [0, 1, 2].map(i => Math.min(...list.map(b => b.min[i]))) : null,
    max: list.length ? [0, 1, 2].map(i => Math.max(...list.map(b => b.max[i]))) : null,
    flat: flat ? flat.flat_size : null,
    suppressed: new Map(((features && features.features) || []).filter(f => !/sketch/i.test(f.type || "")).map(f => [f.name.toLowerCase(), f.status === "suppressed"])),
    dimensions: new Map(((dimensions && dimensions.dimensions) || []).map(d => [d.address, d.value])),
    unbuilt: failed,
    // The features that make geometry (not sketches, planes or axes), for which part has a cut the other hasn't
    // (a smart component's cut, which FabSys's ray picks the part for)
    solid: new Set(((features && features.features) || []).filter(f => !/sketch|plane|axis|point|coordinate/i.test(f.type || "")).map(f => f.name.toLowerCase())),
  };
}

// ---- The SolidWorks side: exports ----

function readExports(dir, rootName) {
  const byName = new Map();
  for (const f of fs.readdirSync(dir).filter(f => f.endsWith(".swx.json"))) byName.set(f.replace(/\.swx\.json$/i, "").toLowerCase(), path.join(dir, f));
  const load = file => { try { return JSON.parse(fs.readFileSync(file, "utf8")); } catch { return null; } };
  const asmFiles = [...byName.keys()].filter(k => k.endsWith(".sldasm"));
  const rootKey = rootName ? asmFiles.find(k => stem(k) === stem(rootName).toLowerCase()) : asmFiles.find(k => /enclosure assembly/.test(k));
  if (!rootKey) throw new Error(`no root assembly export in ${dir}: give --sw-root`);
  const rootExport = load(byName.get(rootKey));
  const cfg = configurationOf(rootExport, rootExport.activeConfiguration);
  const components = (cfg.components || []).map(c => ({
    path: c.path.split("/"), file: stem(c.file), assembly: /\.sldasm$/i.test(c.file || ""), configuration: c.configuration || null,
    suppressed: c.suppression === 0, hidden: c.hidden === true || c.visible === 0, flexible: c.solving === "flexible",
    fixed: !!c.fixed, matrix: Array.isArray(c.transform) ? matrixOf(c.transform) : null,
  }));
  // Mates: the root's, and each sub-assembly's from its own export, in the configuration its component shows
  const assemblies = [{ file: rootExport.name || stem(rootKey), path: [], configuration: cfg.name, mates: matesOf(cfg) }];
  for (const c of components.filter(c => c.assembly && !c.suppressed)) {
    const exp = byName.get(`${c.file.toLowerCase()}.sldasm`);
    const own = exp && load(exp);
    if (own) assemblies.push({ file: c.file, path: c.path, configuration: c.configuration, mates: matesOf(configurationOf(own, c.configuration)) });
  }
  const exportOf = (file, assembly) => {
    const f = byName.get(`${file.toLowerCase()}.${assembly ? "sldasm" : "sldprt"}`);
    return f ? load(f) : null;
  };
  return { kind: "SolidWorks", dir, root: stem(path.basename(byName.get(rootKey)).replace(/\.swx\.json$/i, "")), rootExport, configuration: cfg, components, assemblies, exportOf };
}

const configurationOf = (exp, name) => (exp.configurations || []).find(c => c.name === name)
  || (exp.configurations || []).find(c => c.name === exp.activeConfiguration) || (exp.configurations || [])[0] || {};

/**
 * An assembly configuration's mates: name, suppressed, and whether SolidWorks has an error on it. A mate of a
 * suppressed component counts as suppressed, as PeetCAD reports it (SolidWorks leaves the mate's own state alone).
 */
function matesOf(cfg) {
  const out = [];
  const off = (cfg.components || []).filter(c => c.suppression === 0).map(c => c.path);
  const ofSuppressed = f => ((f.mate && f.mate.entities) || []).some(e => e && e.component && off.some(p => e.component === p || e.component.startsWith(p + "/")));
  const isMate = f => f.mate || (/^Mate/.test(f.type || "") && f.type !== "MateGroup");
  const walk = list => { for (const f of list || []) { if (isMate(f)) out.push({ name: f.name, suppressed: !!f.suppressed || ofSuppressed(f), error: f.errorCode || null, warning: !!f.isWarning, type: f.mate && f.mate.type, known: true }); walk(f.sub); } };
  walk(cfg.features);
  return out;
}

/** A SolidWorks part in a configuration, from its export (and its STEP and flat DXF beside it). */
function swPart(side, ask, file, configuration) {
  const exp = side.exportOf(file, false);
  if (!exp) return null;
  const cfg = configurationOf(exp, configuration);
  const bodies = cfg.bodies || [];
  let volume = bodies.reduce((a, b) => a + (b.volume || 0), 0) * 1e9;
  let min = bodies.length ? [0, 1, 2].map(i => Math.min(...bodies.map(b => (b.tightBox || b.box)[i])) * MM) : null;
  let max = bodies.length ? [0, 1, 2].map(i => Math.max(...bodies.map(b => (b.tightBox || b.box)[i + 3])) * MM) : null;
  let reference = "SolidWorks' mass properties";
  const step = cfg.step ? measureStep(ask, path.join(side.dir, cfg.step)) : null;
  if (step) { volume = step.volume; min = step.min; max = step.max; reference = "SolidWorks' STEP"; }
  // The flat size: its flat DXF, else its flat-pattern configuration's body if that is unfolded
  let flat = cfg.flatDxf ? dxfSize(path.join(side.dir, cfg.flatDxf)) : null;
  if (!flat) {
    const fp = (exp.configurations || []).find(c => c.flatPattern && c.name.toLowerCase() === `${cfg.name}sm-flat-pattern`.toLowerCase());
    const b = fp && fp.unfolded !== false && (fp.bodies || [])[0];
    if (b) flat = [0, 1, 2].map(i => ((b.tightBox || b.box)[i + 3] - (b.tightBox || b.box)[i]) * MM).sort((p, q) => q - p).slice(0, 2).map(tidy);
  }
  const suppressed = new Map();
  const dimensions = new Map();
  const walk = list => {
    for (const f of list || []) {
      // Sketches are left out: SolidWorks counts one suppressed when the feature that uses it is
      if (!/ProfileFeature/i.test(f.type || "")) suppressed.set(f.name.toLowerCase(), !!f.suppressed);
      for (const d of f.dimensions || []) {
        if (typeof d.value !== "number" || !d.fullName) continue;
        // By address as written: "D1@LPattern2" (a count) and PeetCAD's "d1@LPattern2" (a spacing) are two
        const address = d.fullName.split("@").slice(0, 2).join("@");
        // swDimensionParamType_e: 0 length (metres), 1 angle (radians), 2 a count
        dimensions.set(address, d.valueType === 1 ? d.value * 180 / Math.PI : d.valueType === 0 ? d.value * MM : d.value);
      }
      walk(f.sub);
    }
  };
  walk(cfg.features);
  // The features that made a face of the body
  const made = new Set(bodies.flatMap(b => (b.faces || []).map(f => f.feature)).filter(Boolean).map(n => n.toLowerCase()));
  return { volume, min, max, flat, reference, suppressed, dimensions, made, configuration: cfg.name };
}

// ---- The SolidWorks side: a dry run ----

/**
 * The tree a dry run of the job says SolidWorks would build, in SolidWorks' names (SolidWorksManager): a part
 * "<JobNumber> <Prefix> <Name>", an assembly "<JobNumber> <Name>", each instance "<file>-<n>"; the components of a
 * template assembly FabSys copies each copied as "<JobNumber>[ <Prefix>] <resolved name or template name>" unless
 * the script suppresses it. No placements.
 */
function readDryRun(file, templatesExports) {
  const report = JSON.parse(fs.readFileSync(file, "utf8"));
  const tree = report.dry_run && report.dry_run.tree;
  if (!tree) throw new Error(`${file} has no dry run tree: run FabSys.Run --backend dryrun with a build that writes it`);
  const job = report.job_number || null;
  const components = [];
  const assemblies = [];
  const notes = [];
  const operations = mateOperations(report.dry_run.operations);
  const exportOf = name => {
    if (!templatesExports) return null;
    const f = fs.readdirSync(templatesExports).find(x => x.toLowerCase() === `${name.toLowerCase()}.sldasm.swx.json`);
    return f ? JSON.parse(fs.readFileSync(path.join(templatesExports, f), "utf8")) : null;
  };
  const counter = () => { const n = new Map(); return f => { const k = f.toLowerCase(); n.set(k, (n.get(k) || 0) + 1); return n.get(k); }; };
  const prefixOf = node => node.SheetMetalConfig ? node.SheetMetalConfig.Prefix : null;
  // What the script called it: its name, less the dry run's own "-<n>" where it made one up
  const calledOf = node => {
    const [base, n] = splitInstance(node.Name);
    return n !== null && base === node.TemplateName ? base : node.Name;
  };
  const fileOf = node => {
    if (node.InsertMethod === "CreateSubAssembly") return node.Name;
    const called = calledOf(node);
    if (node.IsAssembly) return job && called.startsWith(job + " ") ? called : `${job} ${called}`;
    return `${job} ${prefixOf(node)} ${called}`;
  };
  // A template assembly's own components, copied as FabSys copies them (copyTemplate), under `parentPath`
  const copyTemplate = (node, templateName, configuration, parentPath, mates, seen, inEquation = false) => {
    const exp = exportOf(templateName);
    if (!exp) { notes.push(`no export of the template ${templateName}: its components are not expected`); return; }
    const cfg = configurationOf(exp, configuration);
    const number = counter();
    // A pattern's copies (SolidWorks lists each as a ReferencePattern feature) follow their seed: when FabSys swaps
    // the seed for its copy they become instances of the copy, which FabSys then leaves ("already resolved")
    const copies = new Set();
    (function walk(list) { for (const f of list || []) { if (f.type === "ReferencePattern") copies.add(f.name); walk(f.sub); } })(cfg.features);
    const own = (cfg.components || []).filter(c => !c.path.includes("/"))
      .sort((a, b) => copies.has(a.path) - copies.has(b.path) || (splitInstance(a.path)[1] || 0) - (splitInstance(b.path)[1] || 0));
    const seedFile = new Map();
    // Components the template's equations suppress or not ("Hinge Assembly-3.Assembly" = IIF(...)): FabSys
    // unsuppresses every component it copies, and the equation decides again when the copy is rebuilt, from
    // dimensions a dry run doesn't work out
    const byEquation = new Set((cfg.equations || []).map(e => /^\s*"([^"]+)\.(Assembly|Part)"\s*=/i.exec(e.expression || "")).filter(Boolean).map(m => m[1].toLowerCase()));
    for (const c of own) {
      const isAsm = /\.sldasm$/i.test(c.file || "");
      const [base] = splitInstance(c.path);
      const keyFull = [...seen, c.path].join("/");
      if (node.SuppressedComponents.includes(c.path) || node.SuppressedComponents.includes(keyFull)) {
        components.push({ path: [...parentPath, c.path], file: stem(c.file), assembly: isAsm, configuration: c.configuration, suppressed: true, hidden: false, matrix: null });
        continue;
      }
      // Already the job's own file (a pattern's copy): FabSys leaves it
      if (job && base.startsWith(job)) {
        components.push({ path: [...parentPath, c.path], file: stem(c.file), assembly: isAsm, configuration: c.configuration, suppressed: c.suppression === 0, hidden: c.hidden === true, matrix: null });
        continue;
      }
      const smc = node.PartSheetMetalConfigs[c.path] || node.PartSheetMetalConfigs[keyFull] || node.SheetMetalConfig;
      const resolved = node.ResolvedComponents[c.path] || node.ResolvedComponents[keyFull] || base;
      const template = stem(c.file).toLowerCase();
      const fileName = copies.has(c.path) && seedFile.has(template) ? seedFile.get(template)
        : isAsm ? `${job} ${resolved}` : `${job} ${smc ? smc.Prefix : prefixOf(node)} ${resolved}`;
      if (!copies.has(c.path) && !seedFile.has(template)) seedFile.set(template, fileName);
      const instance = `${fileName}-${number(fileName)}`;
      const configurationShown = node.SubComponentConfigs[c.path] || node.SubComponentConfigs[keyFull] || c.configuration;
      const equation = inEquation || byEquation.has(c.path.toLowerCase());
      components.push({ path: [...parentPath, instance], file: fileName, assembly: isAsm, configuration: configurationShown, suppressed: false, hidden: c.hidden === true, matrix: null, ...(equation ? { byEquation: true } : {}) });
      if (isAsm) copyTemplate(node, stem(c.file), configurationShown, [...parentPath, instance], null, [...seen, c.path], equation);
    }
    if (mates) mates.push(...matesOf(cfg).filter(m => !m.suppressed).map(m => ({ name: m.name })));
  };
  const walk = (node, parentPath, number, names, mates) => {
    for (const child of node.Children || []) {
      const file = fileOf(child);
      const instance = `${file}-${number(file)}`;
      names.push([child.Name, instance]);
      const p = [...parentPath, instance];
      components.push({
        path: p, file, assembly: child.IsAssembly, configuration: child.ConfigurationName || null, suppressed: false,
        hidden: !!child.IsHidden, flexible: !!child.IsFlexible, matrix: null, features: child.FeatureSuppressions,
      });
      if (child.IsAssembly) {
        const own = [];
        const ownNames = [];
        if (child.TemplateName) copyTemplate(child, child.TemplateName, child.ConfigurationName, p, own, []);
        walk(child, p, counter(), ownNames, own);
        assemblies.push({ file, path: p, configuration: child.ConfigurationName, mates: mateNames(child, ownNames, operations).concat(own) });
      }
    }
  };
  const rootNames = [];
  walk(tree, [], counter(), rootNames, null);
  assemblies.unshift({ file: "(root)", path: [], configuration: null, mates: mateNames(tree, rootNames, operations) });
  return { kind: "dry run", dir: path.dirname(file), root: "(root)", job, components, assemblies, notes, exportOf: () => null };
}

/**
 * The dry run's operations, for telling apart components the dry run gives one name (six "Plinth bolt"s, where
 * SolidWorks returns "<file>-1" to "<file>-6"): each AddMate with how many components of each name had been
 * inserted by then. The script mates the one it has just inserted.
 */
function mateOperations(operations) {
  const inserted = new Map();
  const out = [];
  for (const op of operations || []) {
    let m = /^InsertComponent: Insert '.*' as '(.*)'$/.exec(op) || /^SaveAndInsertSubAssembly: SaveAndInsert sub-assembly '(.*)'$/.exec(op);
    if (m) { inserted.set(m[1], (inserted.get(m[1]) || 0) + 1); continue; }
    m = /^AddMate: AddMate (\w+) between '(.*)' and '(.*)'$/.exec(op);
    if (m) out.push({ type: m[1], a: m[2], b: m[3], counts: new Map([[m[2], inserted.get(m[2]) || 0], [m[3], inserted.get(m[3]) || 0]]), used: false });
  }
  return out;
}

/** The names FabSys gives the mates of a dry run's assembly, with its components' dry-run names as SolidWorks' instances. */
function mateNames(node, names, operations = []) {
  const type = t => typeof t === "string" ? t : ["Coincident", "Concentric", "Perpendicular", "Parallel", "Tangent", "Distance", "Angle"][t] || String(t);
  // Each dry-run name stands for the instances it was given, in order
  const queue = new Map();
  for (const [dry, instance] of names) { if (!queue.has(dry)) queue.set(dry, []); queue.get(dry).push(instance); }
  const dryNames = [...queue.keys()].sort((a, b) => b.length - a.length);
  const instanceOf = (comp, counts) => {
    if (!comp) return comp;
    const dry = dryNames.find(d => comp.includes(d));
    if (!dry) return comp;
    const list = queue.get(dry);
    const k = counts && counts.get(comp) ? Math.min(counts.get(comp), list.length) : 1;
    return comp.split(dry).join(list[k - 1]);
  };
  const given = new Set();
  return (node.Mates || []).filter(m => !m.IsSuppressed).map(m => {
    // The operation that added it, in order
    const op = operations.find(o => !o.used && o.type === type(m.MateType) && o.a === m.Entity1Component && o.b === m.Entity2Component);
    if (op) op.used = true;
    const counts = op && op.counts;
    let name = `${type(m.MateType)}_`;
    if (m.Entity1Component) name += `${instanceOf(m.Entity1Component, counts)}_`;
    name += `${m.Entity1Name}_`;
    if (m.Entity2Component) name += `${instanceOf(m.Entity2Component, counts)}_`;
    name += `${m.Entity2Name}_`;
    // SolidWorks won't give two features one name: the second keeps its own ("Coincident12")
    if (given.has(name.toLowerCase())) return { name: type(m.MateType), defaultName: true };
    given.add(name.toLowerCase());
    return { name };
  });
}

// ---- Comparing ----

function compareJob(sw, peet, { ask, swJob, peetJob, tolerance = 1e-3, edited = new Set(), parts = true } = {}) {
  const swJobN = swJob !== undefined ? swJob : (sw.job || jobOf(sw.components.map(c => c.file)));
  // PeetCAD's job number: the first word most of its files share, or else the other side's where most of its files
  // start with that ("Test 1", "XXXXX": job numbers with no digit, or with a space)
  const sharing = job => job && peet.components.filter(c => c.file.startsWith(job + " ")).length > peet.components.length / 2;
  const peetJobN = peetJob !== undefined ? peetJob : jobOf(peet.components.map(c => c.file)) || (sharing(swJobN) ? swJobN : null);
  const swKey = c => keyOf(c.path, swJobN), peetKey = c => keyOf(c.path, peetJobN);
  const peetByKey = new Map(peet.components.map(c => [peetKey(c), c]));
  const pairs = [];
  const swLeft = [];
  const used = new Set();
  for (const c of sw.components) {
    const p = peetByKey.get(swKey(c));
    if (p) { pairs.push([c, p, "name"]); used.add(p); } else swLeft.push(c);
  }
  // Instances numbered differently: same parent, same file, paired by nearest placement
  const fileKey = (c, job) => withoutJob(c.file, job).toLowerCase();
  const parentKey = (c, job) => keyOf(c.path.slice(0, -1), job);
  const missing = [];
  for (const c of swLeft) {
    const options = peet.components.filter(p => !used.has(p) && fileKey(p, peetJobN) === fileKey(c, swJobN) && parentKey(p, peetJobN) === parentKey(c, swJobN));
    const best = options.map(p => [p, c.matrix && p.matrix ? apart(c.matrix, p.matrix) : 0]).sort((a, b) => a[1] - b[1])[0];
    if (best) { pairs.push([c, best[0], "file"]); used.add(best[0]); } else missing.push(c);
  }
  const extra = peet.components.filter(p => !used.has(p));

  const partCache = new Map();
  const partOf = (side, c) => {
    const k = `${side.kind}|${c.file}|${c.configuration}`;
    if (!partCache.has(k)) partCache.set(k, side.kind === "PeetCAD" ? (c.linkFile ? peetPart(ask, c.linkFile, c.configuration) : null) : swPart(side, ask, c.file, c.configuration));
    return partCache.get(k);
  };

  const rows = [];
  for (const [s, p, how] of pairs) {
    const problems = [];
    const row = { component: s.path.join("/"), peetcad: p.path.join("/"), file: s.file, matched_by: how };
    if (how === "file") problems.push(`numbered ${splitInstance(p.path[p.path.length - 1])[1]} in PeetCAD, ${splitInstance(s.path[s.path.length - 1])[1]} in SolidWorks`);
    if (edited.has(withoutJob(s.file, swJobN).toLowerCase())) row.edited_after_run = true;
    if (s.suppressed !== p.suppressed) {
      const what = s.suppressed ? "suppressed in SolidWorks, not in PeetCAD" : "suppressed in PeetCAD, not in SolidWorks";
      if (s.byEquation) row.note = `${what}: a template equation decides, which the dry run doesn't work out`;
      else problems.push(what);
    }
    if (!!s.hidden !== !!p.hidden) problems.push(s.hidden ? "hidden in SolidWorks, shown in PeetCAD" : "hidden in PeetCAD, shown in SolidWorks");
    if (s.configuration && p.configuration && s.configuration.toLowerCase() !== p.configuration.toLowerCase()) problems.push(`configuration ${p.configuration} in PeetCAD, ${s.configuration} in SolidWorks`);
    if (s.configuration && !p.configuration && p.configurations && p.configurations.length > 1) problems.push(`configuration not known in PeetCAD, ${s.configuration} in SolidWorks`);
    row.configuration = { solidworks: s.configuration, peetcad: p.configuration };
    if (s.flexible !== undefined && s.assembly && !!s.flexible !== !!p.flexible) problems.push(s.flexible ? "flexible in SolidWorks, rigid in PeetCAD" : "flexible in PeetCAD, rigid in SolidWorks");
    // A sub-assembly is placed by its parts, which are compared: SolidWorks' transform of a sub-assembly
    // component need not be where its parts are (import-asm.js, anchorOf)
    if (!s.suppressed && !p.suppressed && s.matrix && p.matrix && !s.assembly) {
      row.off_mm = tidy(apart(s.matrix, p.matrix));
      if (row.off_mm > tolerance) problems.push(`placed ${row.off_mm} mm from SolidWorks' placement`);
    }
    if (p.status && p.status !== "ok" && p.status !== "suppressed") { row.peetcad_status = p.status; if (p.message) row.peetcad_message = p.message; problems.push(`PeetCAD: ${p.message || p.status}`); }
    if (parts && !s.assembly && !s.suppressed && !p.suppressed) {
      const a = partOf(sw, s), b = partOf(peet, p);
      if (a && b && !b.error) {
        row.part = comparePart(a, b);
        if (row.part.verdict !== "matches") problems.push(`part ${row.part.verdict}`);
      } else if (b && b.error) problems.push(`PeetCAD's part can't be read: ${b.error}`);
    }
    row.problems = problems;
    row.verdict = problems.length ? "differs" : "matches";
    rows.push(row);
  }
  for (const c of missing) {
    // Under a sub-assembly PeetCAD has suppressed by a template equation (a dry run doesn't evaluate those)
    const underSuppressed = c.byEquation && pairs.some(([s, p]) => p.suppressed && c.path.join("/").startsWith(s.path.join("/") + "/"));
    rows.push({
      component: c.path.join("/"), file: c.file,
      verdict: underSuppressed ? "not compared (a template equation suppresses its sub-assembly in PeetCAD)" : c.suppressed ? "missing in PeetCAD (suppressed in SolidWorks)" : "missing in PeetCAD",
      ...(edited.has(withoutJob(c.file, swJobN).toLowerCase()) ? { edited_after_run: true } : {}),
    });
  }
  for (const p of extra) rows.push({ component: null, peetcad: p.path.join("/"), file: p.file, verdict: p.suppressed ? "extra in PeetCAD (suppressed there)" : "extra in PeetCAD" });

  // Mates, assembly by assembly (the root, and each sub-assembly both sides have), by name
  const mates = [];
  for (const a of sw.assemblies) {
    const k = keyOf(a.path, swJobN);
    const b = peet.assemblies.find(x => keyOf(x.path, peetJobN) === k);
    if (!b) continue;
    const peetMates = new Map((b.mates || []).map(m => [withoutJob(m.name, peetJobN).toLowerCase(), m]));
    const out = { assembly: a.path.join("/") || "(root)", solidworks: a.mates.length, peetcad: (b.mates || []).length, both: 0, solve_differently: [], missing: [], extra: [] };
    const seen = new Set();
    for (const m of [...a.mates].sort((x, y) => !!x.defaultName - !!y.defaultName)) {
      // A mate with the name SolidWorks gave it ("Coincident12") is any such mate the other side has left over
      const key = m.defaultName ? [...peetMates.keys()].find(k => !seen.has(k) && new RegExp(`^${m.name.toLowerCase()}\\d+$`).test(k)) : withoutJob(m.name, swJobN).toLowerCase();
      const pm = key && peetMates.get(key);
      if (!pm) { out.missing.push(m.defaultName ? `${m.name}<n> (a second mate of the same name)` : m.name); continue; }
      seen.add(key);
      out.both++;
      const peetState = pm.status === "ok" ? "ok" : pm.status === "suppressed" ? "suppressed" : "error";
      // A dry run says which mates there are, not whether they solve or are suppressed: only PeetCAD's failures count
      const differs = m.known ? (m.suppressed ? "suppressed" : m.error ? "error" : "ok") !== peetState : peetState === "error";
      if (differs)
        out.solve_differently.push({ mate: m.name, solidworks: !m.known ? "(dry run)" : m.suppressed ? "suppressed" : m.error ? `error ${m.error}` : "ok", peetcad: pm.status, ...(pm.message ? { message: pm.message } : {}) });
    }
    // Against a dry run, a mate PeetCAD has suppressed is not extra: the dry run can't tell which of a template's
    // mates its configuration suppresses
    for (const [key, m] of peetMates) if (!seen.has(key) && !(sw.kind === "dry run" && m.status === "suppressed")) out.extra.push({ mate: m.name, status: m.status });
    out.verdict = out.missing.length || out.extra.length || out.solve_differently.length ? "differs" : "matches";
    mates.push(out);
  }

  // The whole assembly
  let mass = null;
  if (peet.mass && sw.kind === "SolidWorks") {
    const cfg = sw.configuration;
    const step = cfg.step ? measureStep(ask, path.join(sw.dir, cfg.step)) : null;
    const mp = cfg.massProperties;
    // The volume: SolidWorks' STEP of the configuration, measured by PeetCAD (its mass properties can be stale:
    // import-asm.js). The box: the bodies' extreme points SolidWorks gives (`tightBox`), as PeetCAD's box is the
    // bodies' too. The STEP's box is not used: SolidWorks writes a flexible sub-assembly into it as the
    // sub-assembly's own file has it (Door Assembly's hinges), so its box can be larger than the assembly
    const mpBox = mp && (mp.tightBox || mp.box);
    const volume = step ? step.volume : mp && typeof mp.volume === "number" ? mp.volume * 1e9 : null;
    const box = Array.isArray(mpBox) && mpBox.length >= 6 ? { min: mpBox.slice(0, 3).map(v => v * MM), max: mpBox.slice(3, 6).map(v => v * MM), reference: mp.tightBox ? "SolidWorks' extreme points" : "SolidWorks' box" }
      : step ? { min: step.min, max: step.max, reference: "SolidWorks' STEP" } : null;
    if (volume) {
      mass = {
        reference: step ? "SolidWorks' STEP" : "SolidWorks' mass properties", volume_mm3: tidy(peet.mass.volume_mm3), sw_volume_mm3: tidy(volume), volume_ratio: tidy(peet.mass.volume_mm3 / volume),
        ...(step && mp && typeof mp.volume === "number" ? { sw_mass_properties_mm3: tidy(mp.volume * 1e9) } : {}),
      };
      if (box && peet.mass.min) {
        Object.assign(mass, { min: peet.mass.min, max: peet.mass.max, sw_min: box.min.map(tidy), sw_max: box.max.map(tidy), box_reference: box.reference,
          box_mm: tidy(Math.max(...[0, 1, 2].flatMap(i => [Math.abs(peet.mass.min[i] - box.min[i]), Math.abs(peet.mass.max[i] - box.max[i])]))) });
      }
      mass.verdict = Math.abs(mass.volume_ratio - 1) < 1e-4 && !(mass.box_mm > 1e-2) ? "matches" : "differs";
    }
  }

  const count = v => rows.filter(r => r.verdict === v).length;
  const summary = {
    components: { solidworks: sw.components.length, peetcad: peet.components.length, matching: count("matches"), differing: count("differs"),
      missing: rows.filter(r => /^missing/.test(r.verdict)).length, extra: rows.filter(r => /^extra/.test(r.verdict)).length,
      placed_off: rows.filter(r => r.off_mm > tolerance).length, worst_off_mm: tidy(Math.max(0, ...rows.map(r => r.off_mm || 0))),
      parts_differing: rows.filter(r => r.part && r.part.verdict !== "matches").length,
      edited_after_run: rows.filter(r => r.edited_after_run).length },
    mates: { solidworks: mates.reduce((a, m) => a + m.solidworks, 0), peetcad: mates.reduce((a, m) => a + m.peetcad, 0), both: mates.reduce((a, m) => a + m.both, 0),
      solve_differently: mates.reduce((a, m) => a + m.solve_differently.length, 0) },
  };
  const verdict = summary.components.differing || summary.components.missing || summary.components.extra || mates.some(m => m.verdict !== "matches") || (mass && mass.verdict !== "matches")
    ? "differs" : "matches";
  return {
    solidworks: { kind: sw.kind, dir: sw.dir, root: sw.root, job: swJobN }, peetcad: { dir: peet.dir, root: peet.root, job: peetJobN },
    verdict, summary, mass, components: rows, mates, notes: sw.notes || [],
  };
}

/** One part against another: volume, box, flat size, features' suppression, dimensions. */
function comparePart(a, b) {
  const out = { reference: a.reference };
  const problems = [];
  if (a.volume > 0 && b.volume !== undefined) {
    out.volume_mm3 = tidy(b.volume); out.sw_volume_mm3 = tidy(a.volume); out.volume_ratio = tidy(b.volume / a.volume);
    if (Math.abs(out.volume_ratio - 1) > 1e-4) problems.push(`volume x${out.volume_ratio}`);
  }
  if (a.min && b.min) {
    out.box_mm = tidy(Math.max(...[0, 1, 2].flatMap(i => [Math.abs(a.min[i] - b.min[i]), Math.abs(a.max[i] - b.max[i])])));
    if (out.box_mm > 1e-2) problems.push(`box ${out.box_mm} mm off`);
  }
  if (a.flat && b.flat) {
    const p = [...a.flat].sort((x, y) => x - y), q = [...b.flat].sort((x, y) => x - y);
    out.flat = { solidworks: a.flat, peetcad: b.flat };
    out.flat_mm = tidy(Math.max(...p.map((x, i) => Math.abs(x - q[i]))));
    if (out.flat_mm > 1e-2) problems.push(`flat size ${out.flat_mm} mm off`);
  }
  const features = [];
  for (const [name, s] of a.suppressed) if (b.suppressed.has(name) && b.suppressed.get(name) !== s) features.push(`${name}: ${s ? "suppressed" : "unsuppressed"} in SolidWorks`);
  if (features.length) { out.feature_suppression = features; problems.push(`${features.length} features suppressed differently`); }
  const dims = [];
  for (const [address, v] of a.dimensions) if (b.dimensions.has(address) && Math.abs(b.dimensions.get(address) - v) > 1e-3) dims.push({ dimension: address, solidworks: tidy(v), peetcad: tidy(b.dimensions.get(address)) });
  if (dims.length) { out.dimensions = dims; problems.push(`${dims.length} dimensions differ`); }
  if (b.unbuilt && b.unbuilt.length) out.peetcad_unbuilt = b.unbuilt;
  // Features one side has and the other hasn't (by name): where a smart component's cut went, a feature the
  // import left out. Told, not counted: the volume and box say whether they matter
  if (a.made && a.made.size && b.solid) {
    const onlySw = [...a.made].filter(n => !b.solid.has(n) && !b.suppressed.has(n));
    const onlyPeet = [...b.solid].filter(n => !a.suppressed.has(n) && b.suppressed.get(n) === false);
    if (onlySw.length) out.features_only_in_solidworks = onlySw;
    if (onlyPeet.length) out.features_only_in_peetcad = onlyPeet;
  }
  out.verdict = problems.length ? `differs: ${problems.join(", ")}` : "matches";
  return out;
}

// ---- Main ----

function main(argv) {
  const args = [...argv];
  const option = name => { const i = args.indexOf(name); if (i < 0) return undefined; const v = args[i + 1]; args.splice(i, 2); return v; };
  const flag = name => { const i = args.indexOf(name); if (i < 0) return false; args.splice(i, 1); return true; };
  const repo = path.resolve(__dirname, "..", "..");
  const peetExe = option("--peet") || [path.join(repo, "target", "release", "peet.exe"), path.join(repo, "target", "debug", "peet.exe")].find(p => fs.existsSync(p)) || "peet";
  const out = option("--out");
  const swRoot = option("--sw-root"), peetRoot = option("--peet-root");
  const swJob = option("--sw-job"), peetJob = option("--peet-job");
  const templatesExports = option("--templates-exports");
  const editedFile = option("--edited");
  const tolerance = option("--tolerance");
  const noParts = flag("--no-parts");
  if (args.length !== 2) {
    console.error("usage: compare-job.js <SolidWorks exports folder | dry run report.json> <PeetCAD job folder> [--out report.json] [--peet peet] [--sw-root name] [--peet-root name] [--sw-job n] [--peet-job n] [--templates-exports folder] [--edited list.txt] [--tolerance mm] [--no-parts]");
    return 2;
  }
  const [swSide, peetSide] = args.map(a => path.resolve(a));
  const ask = peetRunner(peetExe);
  const sw = fs.statSync(swSide).isDirectory() ? readExports(swSide, swRoot) : readDryRun(swSide, templatesExports && path.resolve(templatesExports));
  const peet = readPeet(peetSide, ask, peetRoot, sw.configuration && sw.configuration.name);
  const edited = new Set(editedFile ? fs.readFileSync(editedFile, "utf8").split(/\r?\n/).map(l => l.trim()).filter(Boolean).map(l => withoutJob(stem(l), swJob || sw.job || jobOf(sw.components.map(c => c.file))).toLowerCase()) : []);
  const report = compareJob(sw, peet, { ask, swJob, peetJob, tolerance: tolerance ? +tolerance : 1e-3, edited, parts: !noParts });
  if (out) fs.writeFileSync(out, JSON.stringify(report, null, 1));
  printReport(report);
  return report.verdict === "matches" ? 0 : 1;
}

function printReport(r) {
  const s = r.summary;
  console.log(`${r.verdict.toUpperCase()}: ${r.solidworks.kind} ${r.solidworks.root} (job ${r.solidworks.job}) against PeetCAD ${r.peetcad.root} (job ${r.peetcad.job})`);
  console.log(`components: ${s.components.solidworks} in ${r.solidworks.kind}, ${s.components.peetcad} in PeetCAD; ${s.components.matching} match, ${s.components.differing} differ, ${s.components.missing} missing in PeetCAD, ${s.components.extra} extra; ${s.components.placed_off} placed off (worst ${s.components.worst_off_mm} mm); ${s.components.parts_differing} parts differ${s.components.edited_after_run ? `; ${s.components.edited_after_run} edited by hand after the run` : ""}`);
  console.log(`mates: ${s.mates.solidworks} in ${r.solidworks.kind}, ${s.mates.peetcad} in PeetCAD, ${s.mates.both} on both, ${s.mates.solve_differently} solve differently`);
  if (r.mass) console.log(`assembly: volume x${r.mass.volume_ratio} of ${r.mass.reference}, box ${r.mass.box_mm} mm off: ${r.mass.verdict}`);
  for (const c of r.components.filter(c => c.verdict !== "matches")) {
    console.log(`  ${c.verdict.padEnd(14)} ${c.component || c.peetcad}${c.edited_after_run ? " (edited after the run)" : ""}${c.problems && c.problems.length ? ": " + c.problems.join("; ") : ""}`);
  }
  for (const m of r.mates.filter(m => m.verdict !== "matches")) {
    console.log(`  mates of ${m.assembly}: ${m.missing.length} missing in PeetCAD, ${m.extra.length} extra, ${m.solve_differently.length} solve differently`);
    for (const x of m.missing.slice(0, 20)) console.log(`    missing ${x}`);
    for (const x of m.extra.slice(0, 20)) console.log(`    extra   ${x.mate} (${x.status})`);
    for (const x of m.solve_differently.slice(0, 20)) console.log(`    ${x.mate}: SolidWorks ${x.solidworks}, PeetCAD ${x.peetcad}${x.message ? ` (${x.message.slice(0, 120)})` : ""}`);
  }
  for (const n of r.notes) console.log(`  note: ${n}`);
}

module.exports = { compareJob, readPeet, readExports, readDryRun, peetRunner, comparePart, withoutJob, jobOf, keyOf, matrixFromPeet, matrixOf, apart };

if (require.main === module) process.exitCode = main(process.argv.slice(2));
