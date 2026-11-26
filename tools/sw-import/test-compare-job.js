#!/usr/bin/env node
/**
 * Tests compare-job.js: a small job made in PeetCAD by `peet` (a plate, two pins and a sub-assembly of two pins, job
 * number J1, named as FabSys names files) against hand-made SolidWorks exports of it (job number J2, which the
 * comparison takes out) and a hand-made dry run report: the same job matches; a component moved, one missing, a
 * configuration, a mate's state and a part's volume that differ are each found; instances numbered differently (as
 * SolidWorks numbers a second FabSys run into one assembly) are paired by their file, in order or by where they are,
 * a renumbered sub-assembly's components under it, and mates through the pairing, by name or by what they mate.
 *
 *   node tools/sw-import/test-compare-job.js [--peet <peet>]
 */
const fs = require("fs");
const os = require("os");
const path = require("path");
const assert = require("assert");
const { spawnSync } = require("child_process");
const { compareJob, readPeet, readExports, readDryRun, peetRunner } = require("./compare-job");

const args = process.argv.slice(2);
const at = args.indexOf("--peet");
const repo = path.resolve(__dirname, "..", "..");
const peetExe = at >= 0 ? args[at + 1]
  : [path.join(repo, "target", "release", "peet.exe"), path.join(repo, "target", "debug", "peet.exe")].find(p => fs.existsSync(p)) || "peet";

const dir = fs.mkdtempSync(path.join(os.tmpdir(), "compare-job-"));
if (process.env.KEEP) console.log(dir);
const peetDir = path.join(dir, "peet"), swDir = path.join(dir, "sw");
fs.mkdirSync(peetDir); fs.mkdirSync(swDir);
const run = (file, ops) => {
  const r = spawnSync(peetExe, ["run", "-", "--new", "-f", file, "-q"], { input: ops.map(o => JSON.stringify(o)).join("\n") + "\n", encoding: "utf8" });
  assert.strictEqual(r.status, 0, `peet failed on ${path.basename(file)}: ${r.stderr}${r.stdout}`);
};

// ---- The PeetCAD job ----
const plate = path.join(peetDir, "J1 MS2 Plate.peet"), pin = path.join(peetDir, "J1 MS2 Pin.peet");
run(plate, [
  { op: "sketch", on: "top", draw: [{ type: "rectangle", from: [0, 0], to: [60, 40] }] },
  { op: "extrude", sketch: "Sketch1", depth: 6 },
  { op: "add_configuration", name: "Other" },
  { op: "configuration", configuration: "Default" },
]);
run(pin, [
  { op: "sketch", on: "top", draw: [{ type: "circle", center: [0, 0], radius: 5 }] },
  { op: "extrude", sketch: "Sketch1", depth: 25 },
]);
// A sub-assembly of two pins, with a mate of FabSys's naming
const bracket = path.join(peetDir, "J1 Bracket Assembly.peet");
const innerMate = "Parallel_J1 MS2 Pin-2_Top Plane_J1 MS2 Pin-1_Top Plane_";
run(bracket, [
  { op: "new", assembly: true, discard: true },
  { op: "insert", path: pin, link: true, fixed: true },
  { op: "insert", path: pin, link: true, at: [20, 0, 0] },
  { op: "mate", type: "parallel", a: { component: "J1 MS2 Pin-2", plane: "top" }, b: { component: "J1 MS2 Pin-1", plane: "top" }, align: "aligned", name: innerMate },
  { op: "save", path: bracket },
]);
const root = path.join(peetDir, "J1 Test Enclosure Assembly.peet");
const mateName = "Coincident_J1 MS2 Pin-1_Top Plane_J1 MS2 Plate-1_Top Plane_";
run(root, [
  { op: "new", assembly: true, discard: true },
  { op: "insert", path: plate, link: true, fixed: true },
  { op: "insert", path: pin, link: true, at: [30, 20, 0] },
  { op: "insert", path: pin, link: true, at: [10, 10, 0] },
  { op: "insert", path: bracket, link: true, at: [0, 0, 50] },
  { op: "mate", type: "coincident", a: { component: "J1 MS2 Pin-1", plane: "top" }, b: { component: "J1 MS2 Plate-1", plane: "top" }, align: "aligned", name: mateName },
  // A mate SolidWorks names itself (FabSys's name was taken): matched by what it mates
  { op: "mate", type: "parallel", a: { component: ["J1 Bracket Assembly-1", "J1 MS2 Pin-1"], plane: "top" }, b: { component: "J1 MS2 Plate-1", plane: "top" }, align: "aligned" },
  { op: "save", path: root },
]);

// ---- SolidWorks' export of the same job, by hand (metres, SolidWorks' row-vector transforms) ----
const transform = (x, y, z) => [1, 0, 0, 0, 1, 0, 0, 0, 1, x / 1000, y / 1000, z / 1000, 1, 0, 0, 0];
const component = (p, file, xyz, extra = {}) => ({ path: p, file: `C:\\Jobs\\J2\\${file}`, configuration: "Default", suppression: 2, fixed: false, transform: transform(...xyz), visible: 1, hidden: false, ...extra });
const PIN = Math.PI * 25 * 25;
const topPlane = c => ({ component: c, entity: { kind: "feature", name: "Top Plane", type: "RefPlane" } });
function swJob({ moved = 0, plateConfiguration = "Default", extra = false, mateSuppressed = false, plateVolume = 14400, numbers = [1, 2], swapped = false, bracketNumber = 1 } = {}) {
  const b = `J2 Bracket Assembly-${bracketNumber}`;
  const components = [
    component("J2 MS2 Plate-1", "J2 MS2 Plate.SLDPRT", [0, 0, 0], { configuration: plateConfiguration, fixed: true }),
    component(`J2 MS2 Pin-${numbers[0]}`, "J2 MS2 Pin.SLDPRT", swapped ? [10, 10, 0] : [30, 20, 0]),
    component(`J2 MS2 Pin-${numbers[1]}`, "J2 MS2 Pin.SLDPRT", swapped ? [30, 20, 0] : [10 + moved, 10, 0]),
    component(b, "J2 Bracket Assembly.SLDASM", [0, 0, 50]),
    component(`${b}/J2 MS2 Pin-1`, "J2 MS2 Pin.SLDPRT", [0, 0, 50]),
    component(`${b}/J2 MS2 Pin-2`, "J2 MS2 Pin.SLDPRT", [20, 0, 50]),
    ...(extra ? [component("J2 MS2 Bolt-1", "J2 MS2 Bolt.SLDPRT", [0, 0, 0])] : []),
  ];
  const pinMate = `J2 MS2 Pin-${numbers[0]}`;
  const mates = [
    { name: `Coincident_${pinMate}_Top Plane_J2 MS2 Plate-1_Top Plane_`, type: "MateCoincident", suppressed: mateSuppressed, errorCode: null, mate: { type: "coincident", entities: [topPlane(pinMate), topPlane("J2 MS2 Plate-1")] } },
    { name: "Parallel7", type: "MateParallel", suppressed: false, errorCode: null, mate: { type: "parallel", entities: [topPlane(`${b}/J2 MS2 Pin-1`), topPlane("J2 MS2 Plate-1")] } },
  ];
  const volume = (14400 + 4 * PIN) / 1e9;
  const exp = {
    formatVersion: 8, name: "J2 Test Enclosure Assembly", activeConfiguration: "Default",
    configurations: [{ name: "Default", components, features: [{ name: "Mates", type: "MateGroup", sub: mates }], massProperties: { volume, tightBox: [-0.005, -0.005, 0, 0.06, 0.04, 0.075] } }],
  };
  const sub = {
    formatVersion: 8, name: "J2 Bracket Assembly", activeConfiguration: "Default",
    configurations: [{
      name: "Default", components: [component("J2 MS2 Pin-1", "J2 MS2 Pin.SLDPRT", [0, 0, 0]), component("J2 MS2 Pin-2", "J2 MS2 Pin.SLDPRT", [20, 0, 0])],
      features: [{ name: "Mates", type: "MateGroup", sub: [{ name: innerMate.replace(/J1/g, "J2"), type: "MateParallel", suppressed: false, errorCode: null, mate: { type: "parallel", entities: [topPlane("J2 MS2 Pin-2"), topPlane("J2 MS2 Pin-1")] } }] }],
    }],
  };
  const part = (name, v, box, configurations = ["Default"]) => ({
    formatVersion: 8, name, activeConfiguration: "Default",
    configurations: configurations.map(c => ({ name: c, features: [{ name: "Boss-Extrude1", type: "Extrusion", suppressed: false, dimensions: [{ fullName: `D1@Boss-Extrude1@${name}.Part`, value: (name.includes("Plate") ? 6 : 25) / 1000, valueType: 0 }] }], bodies: [{ volume: v / 1e9, box, tightBox: box }] })),
  });
  for (const f of fs.readdirSync(swDir)) fs.unlinkSync(path.join(swDir, f));
  fs.writeFileSync(path.join(swDir, "J2 Test Enclosure Assembly.SLDASM.swx.json"), JSON.stringify(exp));
  fs.writeFileSync(path.join(swDir, "J2 Bracket Assembly.SLDASM.swx.json"), JSON.stringify(sub));
  fs.writeFileSync(path.join(swDir, "J2 MS2 Plate.SLDPRT.swx.json"), JSON.stringify(part("J2 MS2 Plate", plateVolume, [0, 0, 0, 0.06, 0.04, 0.006], ["Default", "Other"])));
  fs.writeFileSync(path.join(swDir, "J2 MS2 Pin.SLDPRT.swx.json"), JSON.stringify(part("J2 MS2 Pin", PIN, [-0.005, -0.005, 0, 0.005, 0.005, 0.025])));
}

const ask = peetRunner(peetExe);
const peet = readPeet(peetDir, ask, null);
const compare = () => compareJob(readExports(swDir, null), peet, { ask });
const matesOf = (r, assembly) => r.mates.find(m => m.assembly === assembly);
const show = r => JSON.stringify({ components: r.components.filter(c => c.verdict !== "matches"), mates: r.mates }, null, 1);

// The same job, under another job number: everything matches
swJob();
let r = compare();
assert.strictEqual(r.solidworks.job, "J2");
assert.strictEqual(r.peetcad.job, "J1");
assert.strictEqual(r.summary.components.matching, 6, show(r));
assert.strictEqual(matesOf(r, "(root)").both, 2, show(r));
assert.strictEqual(matesOf(r, "J2 Bracket Assembly-1").both, 1, show(r));
assert.ok(r.mass && r.mass.verdict === "matches", JSON.stringify(r.mass));
assert.strictEqual(r.verdict, "matches", show(r));
const platePart = r.components.find(c => /Plate/.test(c.component)).part;
assert.strictEqual(platePart.verdict, "matches", JSON.stringify(platePart));

// Each difference is found
swJob({ moved: 5, plateConfiguration: "Other", extra: true, mateSuppressed: true, plateVolume: 15000 });
r = compare();
assert.strictEqual(r.verdict, "differs");
const row = text => r.components.find(c => (c.component || "").includes(text));
assert.strictEqual(row("Pin-2").off_mm, 5);
assert.ok(row("Plate").problems.some(p => /configuration Default in PeetCAD, Other in SolidWorks/.test(p)), JSON.stringify(row("Plate")));
assert.ok(/volume/.test(row("Plate").part.verdict), JSON.stringify(row("Plate").part));
assert.strictEqual(row("Bolt").verdict, "missing in PeetCAD");
assert.strictEqual(matesOf(r, "(root)").solve_differently.length, 1);
assert.strictEqual(r.summary.components.missing, 1);

// Instances numbered differently (SolidWorks numbering on from deleted ones) are paired in order: no difference, the
// sub-assembly's components paired under it, its mates and the root's renamed through the pairing
swJob({ numbers: [3, 4], bracketNumber: 2 });
r = compare();
assert.strictEqual(r.verdict, "matches", show(r));
assert.strictEqual(r.summary.components.missing + r.summary.components.extra, 0);
assert.strictEqual(row("Pin-3").peetcad, "J1 MS2 Pin-1");
assert.strictEqual(row("Pin-3").numbered, "1 in PeetCAD, 3 in SolidWorks");
assert.strictEqual(row("Pin-4").peetcad, "J1 MS2 Pin-2");
assert.strictEqual(row("J2 Bracket Assembly-2/J2 MS2 Pin-2").peetcad, "J1 Bracket Assembly-1/J1 MS2 Pin-2");
assert.strictEqual(matesOf(r, "(root)").both, 2, show(r));
assert.strictEqual(matesOf(r, "J2 Bracket Assembly-2").both, 1, show(r));

// ... or by where they are, when that puts more of them where PeetCAD has them
swJob({ numbers: [3, 4], swapped: true });
r = compare();
assert.strictEqual(row("Pin-3").peetcad, "J1 MS2 Pin-2", JSON.stringify(row("Pin-3")));
assert.strictEqual(row("Pin-3").matched_by, "position");
assert.strictEqual(r.summary.components.placed_off, 0);

// A dry run's tree: the names SolidWorks would give, the second pin's mate in its own name
const dryRun = {
  job_number: "J1",
  dry_run: {
    operations: ["InsertComponent: Insert 'Plate' as 'Plate'", "InsertComponent: Insert 'Pin' as 'Pin'", "AddMate: AddMate Coincident between 'Pin' and 'Plate'", "InsertComponent: Insert 'Pin' as 'Pin'"],
    tree: {
      Name: "Root Assembly", IsAssembly: true, InsertMethod: "Root", Children: [
        { Name: "Plate", TemplateName: "Plate", InsertMethod: "Insert", SheetMetalConfig: { Prefix: "MS2" }, Children: [], Mates: [], SuppressedComponents: [] },
        { Name: "Pin", TemplateName: "Pin", InsertMethod: "Insert", SheetMetalConfig: { Prefix: "MS2" }, Children: [], Mates: [], SuppressedComponents: [] },
        { Name: "Pin", TemplateName: "Pin", InsertMethod: "Insert", SheetMetalConfig: { Prefix: "MS2" }, Children: [], Mates: [], SuppressedComponents: [] },
      ],
      Mates: [{ Entity1Component: "Pin", Entity1Name: "Top Plane", Entity2Component: "Plate", Entity2Name: "Top Plane", MateType: 0, IsSuppressed: false }],
    },
  },
};
const dryFile = path.join(dir, "dry.json");
fs.writeFileSync(dryFile, JSON.stringify(dryRun));
r = compareJob(readDryRun(dryFile, null), peet, { ask });
assert.strictEqual(r.summary.components.missing, 0, show(r));
assert.strictEqual(r.summary.components.differing, 0, show(r));
assert.strictEqual(matesOf(r, "(root)").both, 1, show(r));

if (!process.env.KEEP) fs.rmSync(dir, { recursive: true, force: true });
console.log("compare-job: all checks passed");
