#!/usr/bin/env node
/**
 * Writes FabSys template manifests (schema 2, as FabSys's template exporter writes them from SolidWorks) for a
 * PeetCAD template library, so that FabSys's own template validation checks every name a script uses (dimension
 * addresses, global variables, features and planes, named faces and edges, configurations, component instances)
 * against the library instead of against SolidWorks:
 *
 *   node tools/sw-import/manifests.js <library folder> <SolidWorks manifests folder> <output folder> [--peet <peet>]
 *
 * For each SolidWorks manifest whose template the library has (`<library>\<template name>.peet`), the output has a
 * manifest of the same name describing the PeetCAD file, every configuration of it. Its identity (source path and
 * hash, dependencies) is copied from the SolidWorks manifest, so FabSys takes it as describing the template file it
 * would copy. A template the library does not have gets no manifest: FabSys reports it as `manifest-missing`.
 *
 * Point FabSys's corpus at it: FABSYS_CORPUS_MANIFESTS=<output folder>, `dotnet test --filter TestCategory=Corpus`.
 */
const fs = require("fs");
const path = require("path");
const { spawnSync } = require("child_process");

const args = process.argv.slice(2);
const option = name => { const i = args.indexOf(name); if (i < 0) return null; const v = args[i + 1]; args.splice(i, 2); return v; };
const repo = path.resolve(__dirname, "..", "..");
const peetExe = option("--peet")
  || [path.join(repo, "target", "release", "peet.exe"), path.join(repo, "target", "debug", "peet.exe"), "peet"].find(p => p === "peet" || fs.existsSync(p));
if (args.length !== 3) {
  console.error("Usage: node manifests.js <library folder> <SolidWorks manifests folder> <output folder> [--peet <peet>]");
  process.exit(2);
}
const [library, swManifests, outDir] = args.map(a => path.resolve(a));

/** The replies to operations on a file, nothing saved. */
function ask(file, ops) {
  const r = spawnSync(peetExe, ["run", "-", "-f", file, "-q", "--no-save", "--keep-going"],
    { input: ops.map(o => JSON.stringify(o)).join("\n") + "\n", encoding: "utf8", maxBuffer: 1 << 28 });
  return (r.stdout || "").split("\n").filter(l => l.startsWith("{")).map(l => JSON.parse(l));
}

// PeetCAD's names for SolidWorks' feature types where FabSys checks the type (a plane must be a RefPlane)
const TYPES = { "Reference plane": "RefPlane" };
// SolidWorks' standard planes and origin: every part and assembly has them. PeetCAD's are its own (`top` is
// SolidWorks' Front Plane, `front` its Top Plane, `right` its Right Plane), which the FabSys backend maps.
const STANDARD = [["Front Plane", "RefPlane"], ["Top Plane", "RefPlane"], ["Right Plane", "RefPlane"], ["Origin", "OriginProfileFeature"]];

/** An assembly's component instances, at every depth, by SolidWorks' paths ("Hinge Assembly-1/HingeMale-1"). */
function componentsOf(file, configuration, prefix = "", depth = 0) {
  const ops = [...(configuration ? [{ op: "configuration", configuration }] : []), { op: "components" }];
  const replies = ask(file, ops);
  const r = replies[replies.length - 1];
  if (!r || !r.ok) return [];
  const parts = new Map((r.parts || []).map(p => [p.name, p]));
  const out = [];
  for (const c of r.components || []) {
    const part = parts.get(c.part) || {};
    const p = prefix + c.name;
    out.push({
      path: p, filePath: part.link_file || null, referencedConfiguration: c.configuration || part.active_configuration || null,
      suppressionState: c.status === "suppressed" || c.suppressed ? 0 : 2, modelAvailable: true,
    });
    if (part.kind === "assembly" && part.link_file && depth < 6)
      out.push(...componentsOf(part.link_file, c.configuration || null, p + "/", depth + 1));
  }
  return out;
}

/** What a manifest says of one configuration of a library file. */
function configurationOf(file, name, kind, templateName) {
  const queries = [{ op: "features" }, { op: "dimensions" }, { op: "parameters" }];
  if (kind === "part") queries.push({ op: "faces" }, { op: "edges" }, { op: "bodies" });
  else queries.push({ op: "mates" });
  const replies = ask(file, [{ op: "configuration", configuration: name }, ...queries]);
  const [, features, dimensions, parameters, ...rest] = replies;
  const [faces, edges, bodies] = kind === "part" ? rest : [];
  // An assembly's mates are features in SolidWorks' tree, by name
  const mates = kind === "assembly" && rest[0] && rest[0].mates ? rest[0].mates : [];
  const suffix = kind === "part" ? ".Part" : ".Assembly";
  const c = {
    name,
    solidBodyCount: bodies && bodies.ok ? (bodies.bodies || []).length : null,
    features: [
      ...STANDARD.map(([n, t]) => ({ name: n, type: t, suppressed: false, errorCode: 0, isWarning: false })),
      ...((features && features.features) || []).map(f => ({
        name: f.name, type: TYPES[f.type] || f.type, suppressed: !!f.suppressed,
        errorCode: f.status === "failed" ? 1 : 0, isWarning: f.status === "warning",
      })),
      ...mates.map(m => ({
        name: m.name, type: "Mate" + m.type.charAt(0).toUpperCase() + m.type.slice(1), suppressed: m.status === "suppressed",
        errorCode: m.status === "failed" ? 1 : 0, isWarning: false,
      })),
    ],
    dimensions: ((dimensions && dimensions.dimensions) || []).filter(d => d.address).map(d => ({
      fullName: `${d.address}@${templateName}${suffix}`, parameterName: d.address, resolvesViaParameter: true,
      readOnly: false, drivenState: d.driven ? 1 : 2,
    })),
    equations: ((parameters && parameters.parameters) || []).map((p, i) => ({
      index: i, expression: `"${p.name}"= ${p.expression}`, globalVariable: true, disabled: false,
    })),
    namedEntities: [],
    components: [],
    parameterProbes: [],
    featureProbes: [],
  };
  for (const [reply, type, key] of [[faces, "face", "faces"], [edges, "edge", "edges"]]) {
    for (const x of (reply && reply[key]) || []) {
      for (const n of x.names || []) c.namedEntities.push({ name: n, type, bodyIndex: x.body ?? 0 });
    }
  }
  if (kind === "assembly") c.components = componentsOf(file, name);
  return c;
}

fs.mkdirSync(outDir, { recursive: true });
const summary = { written: [], notInLibrary: [] };
for (const f of fs.readdirSync(swManifests).filter(f => f.endsWith(".manifest.json")).sort()) {
  const sw = JSON.parse(fs.readFileSync(path.join(swManifests, f), "utf8").replace(/^﻿/, ""));
  const file = path.join(library, `${sw.templateName}.peet`);
  if (!fs.existsSync(file)) { summary.notInLibrary.push(sw.templateName); continue; }
  const listed = ask(file, [{ op: "status" }, { op: "configurations" }]);
  const status = listed[0] || {};
  const kind = status.kind === "assembly" ? "assembly" : "part";
  const configs = ((listed[1] && listed[1].configurations) || []).map(c => c.name);
  const active = (listed[1] && listed[1].active) || configs[0] || "Default";
  const manifest = {
    schemaVersion: 2,
    exporterVersion: "peetcad-library",
    solidWorksRevision: null,
    probeListSha256: sw.probeListSha256 || null,
    templateName: sw.templateName,
    documentType: sw.documentType,
    sourcePath: sw.sourcePath,
    sourceSha256: sw.sourceSha256,
    exportedUtc: new Date().toISOString(),
    inspectionCompleted: true,
    error: null,
    activeConfiguration: active,
    coverage: {
      features: "complete", displayDimensions: "complete", equations: "complete",
      namedEntities: kind === "part" ? "complete" : "notChecked", components: kind === "assembly" ? "complete" : "notChecked",
    },
    configurations: (configs.length ? configs : [active]).map(n => configurationOf(file, n, kind, sw.templateName)),
    dependencies: sw.dependencies || [],
    peetFile: file,
  };
  fs.writeFileSync(path.join(outDir, f), JSON.stringify(manifest, null, 1));
  summary.written.push(sw.templateName);
  console.log(`${kind.padEnd(9)} ${sw.templateName}  (${manifest.configurations.length} configurations)`);
}
fs.writeFileSync(path.join(outDir, "summary.json"), JSON.stringify(summary, null, 1));
console.log(`${summary.written.length} manifests; not in the library: ${summary.notInLibrary.join(", ") || "none"}`);
