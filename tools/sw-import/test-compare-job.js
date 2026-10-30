#!/usr/bin/env node
/**
 * Tests compare-job.js: a small job made in PeetCAD by `peet` (a plate and two pins, job number J1, named as FabSys
 * names files) against hand-made SolidWorks exports of it (job number J2, which the comparison takes out) and a
 * hand-made dry run report: the same job matches; a component moved, one missing, a configuration, a mate's state
 * and a part's volume that differ are each found; instances numbered differently are paired by their file.
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
const root = path.join(peetDir, "J1 Test Enclosure Assembly.peet");
const mateName = "Coincident_J1 MS2 Pin-1_Top Plane_J1 MS2 Plate-1_Top Plane_";
run(root, [
  { op: "new", assembly: true, discard: true },
  { op: "insert", path: plate, link: true, fixed: true },
  { op: "insert", path: pin, link: true, at: [30, 20, 0] },
  { op: "insert", path: pin, link: true, at: [10, 10, 0] },
  { op: "mate", type: "coincident", a: { component: "J1 MS2 Pin-1", plane: "top" }, b: { component: "J1 MS2 Plate-1", plane: "top" }, align: "aligned", name: mateName },
  { op: "save", path: root },
]);

// ---- SolidWorks' export of the same job, by hand (metres, SolidWorks' row-vector transforms) ----
const transform = (x, y, z) => [1, 0, 0, 0, 1, 0, 0, 0, 1, x / 1000, y / 1000, z / 1000, 1, 0, 0, 0];
const component = (p, file, xyz, extra = {}) => ({ path: p, file: `C:\\Jobs\\J2\\${file}`, configuration: "Default", suppression: 2, fixed: false, transform: transform(...xyz), visible: 1, hidden: false, ...extra });
const PIN = Math.PI * 25 * 25;
function swJob({ moved = 0, plateConfiguration = "Default", extra = false, mateSuppressed = false, plateVolume = 14400, numbers = [1, 2] } = {}) {
  const components = [
    component("J2 MS2 Plate-1", "J2 MS2 Plate.SLDPRT", [0, 0, 0], { configuration: plateConfiguration, fixed: true }),
    component(`J2 MS2 Pin-${numbers[0]}`, "J2 MS2 Pin.SLDPRT", [30, 20, 0]),
    component(`J2 MS2 Pin-${numbers[1]}`, "J2 MS2 Pin.SLDPRT", [10 + moved, 10, 0]),
    ...(extra ? [component("J2 MS2 Bolt-1", "J2 MS2 Bolt.SLDPRT", [0, 0, 0])] : []),
  ];
  const mate = { name: mateName.replace(/J1/g, "J2"), type: "MateCoincident", suppressed: mateSuppressed, errorCode: null, mate: { type: "coincident", entities: [{ component: `J2 MS2 Pin-${numbers[0]}` }, { component: "J2 MS2 Plate-1" }] } };
  const volume = (14400 + 2 * PIN) / 1e9;
  const exp = {
    formatVersion: 8, name: "J2 Test Enclosure Assembly", activeConfiguration: "Default",
    configurations: [{ name: "Default", components, features: [{ name: "Mates", type: "MateGroup", sub: [mate] }], massProperties: { volume, tightBox: [0, 0, 0, 0.06, 0.04, 0.025] } }],
  };
  const part = (name, v, box, configurations = ["Default"]) => ({
    formatVersion: 8, name, activeConfiguration: "Default",
    configurations: configurations.map(c => ({ name: c, features: [{ name: "Boss-Extrude1", type: "Extrusion", suppressed: false, dimensions: [{ fullName: `D1@Boss-Extrude1@${name}.Part`, value: (name.includes("Plate") ? 6 : 25) / 1000, valueType: 0 }] }], bodies: [{ volume: v / 1e9, box, tightBox: box }] })),
  });
  for (const f of fs.readdirSync(swDir)) fs.unlinkSync(path.join(swDir, f));
  fs.writeFileSync(path.join(swDir, "J2 Test Enclosure Assembly.SLDASM.swx.json"), JSON.stringify(exp));
  fs.writeFileSync(path.join(swDir, "J2 MS2 Plate.SLDPRT.swx.json"), JSON.stringify(part("J2 MS2 Plate", plateVolume, [0, 0, 0, 0.06, 0.04, 0.006], ["Default", "Other"])));
  fs.writeFileSync(path.join(swDir, "J2 MS2 Pin.SLDPRT.swx.json"), JSON.stringify(part("J2 MS2 Pin", PIN, [-0.005, -0.005, 0, 0.005, 0.005, 0.025])));
}

const ask = peetRunner(peetExe);
const peet = readPeet(peetDir, ask, null);
const compare = () => compareJob(readExports(swDir, null), peet, { ask });

// The same job, under another job number: everything matches
swJob();
let r = compare();
assert.strictEqual(r.solidworks.job, "J2");
assert.strictEqual(r.peetcad.job, "J1");
assert.strictEqual(r.summary.components.matching, 3, JSON.stringify(r.components, null, 1));
assert.strictEqual(r.mates[0].both, 1);
assert.ok(r.mass && r.mass.verdict === "matches", JSON.stringify(r.mass));
assert.strictEqual(r.verdict, "matches", JSON.stringify(r, null, 1));
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
assert.strictEqual(r.mates[0].solve_differently.length, 1);
assert.strictEqual(r.summary.components.missing, 1);

// Instances numbered differently are paired by their file and where they are
swJob({ numbers: [3, 4] });
r = compare();
assert.strictEqual(r.summary.components.missing, 0);
assert.ok(row("Pin-3").problems.some(p => /numbered 1 in PeetCAD, 3 in SolidWorks/.test(p)), JSON.stringify(row("Pin-3")));
assert.strictEqual(row("Pin-4").peetcad, "J1 MS2 Pin-2");

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
assert.strictEqual(r.verdict, "matches", JSON.stringify(r, null, 1));
assert.strictEqual(r.mates[0].both, 1);

fs.rmSync(dir, { recursive: true, force: true });
console.log("compare-job: all checks passed");
