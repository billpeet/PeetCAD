# peet-sw-export

Writes what a SolidWorks part or assembly is made of as JSON (`<file name>.swx.json`), for
a PeetCAD importer to read and for finding out what PeetCAD can't yet rebuild. It needs
SolidWorks, so it is not part of the Cargo workspace or of CI.

It opens each file read-only in its own SolidWorks instance and never saves it. It refuses
to run if the instance it gets is under user control or has documents open.

```
peet-sw-export.exe <input-list.txt> <output-directory> [--step] [--dxf] [--no-selections] [--unsuppress-always-off]
```

- `input-list.txt`: one path per line (`.sldprt` or `.sldasm`).
- `--step`: also save each configuration of a part as STEP, to compare a rebuilt part with.
- `--dxf`: also save the flat pattern of each configuration of a sheet metal part.
- `--no-selections`: don't read the edges, faces and sketches that feature definitions
  refer to. Reading them rolls the model back to each feature in turn, which is the slow
  part and the one most likely to upset SolidWorks. Use it if an export hangs or fails.
- `--unsuppress-always-off` (parts; off by default): read what a feature suppressed in
  every configuration refers to. SolidWorks gives such a feature's selections as null (an
  edge flange's `Edges: [null]`: Door's `L`/`RInterlockingFlange1`/`2` and
  `L`/`RLockingDoorFlange`), so the importer can't rebuild it. With the flag, in each
  configuration that is not a flat pattern, each such feature (not sketches, planes or
  folders) is unsuppressed in that configuration only (`SetSuppression2`, with the
  suppressed features it is built on, parents first), the part rebuilt, its definition read
  again and the faces it and its sub-features make read, then everything unsuppressed is
  suppressed again, last first, and the part rebuilt. The document stays read-only and is
  never saved. The feature keeps `suppressed: true`; its `definition` is the one read
  unsuppressed, and `readUnsuppressed` says so: `configuration`, `unsuppressed` (names),
  `faces` (as a body's), `errorCode` if it rebuilt with an error, `error` if the read
  failed (then the definition is the suppressed one). Each feature is guarded on its own;
  `export.log` lists what was read. Ignored with `--no-selections`.
- Output: the JSON files, `export.log` and `summary.json`. A document that could not be
  fully exported still gets a file, with `completed` false and the reason.
- Exit code: 0 when every document was fully exported, 1 when one was not, 2 for wrong
  arguments.

## What is in a file

Lengths are metres and angles radians, as the SolidWorks API gives them.

For each configuration:

- `features`: the tree in order, each with its type, suppression, parents and children,
  and its `dimensions` with their values in this configuration.
  - `sketch`: points, segments (lines, arcs, ellipses, splines, and text with its
    `format`: font, bold, italic, height, width factor, spacing), relations with the
    entities and the dimension each one ties (`defined` holds the model edge, vertex or
    plane a relation refers to outside the sketch), what the sketch is on, and the
    transform from model space to the sketch.
  - `definition`: every value the feature's data interface offers (`IExtrudeFeatureData2`,
    `IEdgeFlangeFeatureData`, …), read by reflection, so the names are the API's own and
    are documented in the SolidWorks API help. Methods that take a flag are read both
    ways: `GetDepth(true)`, `GetDepth(false)`. `unreadable` lists what threw.
    The interop assemblies can be older than the SolidWorks the exporter runs on (the
    ones in use are SolidWorks 2018's, revision 26): what they don't know is read by
    name, from the type information the running SolidWorks gives (`dispatchType`), and
    listed in `lateBound`. So a member SolidWorks added later (a linear pattern's
    `D1EndCondition`, from 2019) is written under its own name with no change here, and a
    feature with no data interface in the interop (a corner relief) gets a definition if
    this SolidWorks has one. A few methods are read for each index, from 0 until one
    fails: `GetTransform[i]` (a pattern's instances) and `Get…AtIndex[i]` (add a name to
    `IndexedReads` for more); `GetFaces[i]` for each of a fillet's face sets, 0 to 5
    (`FixedIndexes`). A method whose one argument is a second answer gives it as
    `Name.out` (a reference axis's `GetSelections`). An object with no case of its own is
    written with its `type` and its `values`, two levels deep (a structural member's
    `Groups`).
  - `planeTransform` for a reference plane and `mate` (type, alignment, the entities
    mated) for a mate.
- `equations`, with their values (which of them SolidWorks applies: see format 9).
- `bodies` (parts): volume, area, centre of gravity, bounding box, and every face with its
  surface, the feature that made it and its name if it has one. Named edges too.
- `components` (assemblies): each instance's file, configuration and transform, and for a
  sub-assembly its `solving` ("rigid" or "flexible", SolidWorks' "Solve as"; added
  without a new format version, so an older export has none).

Added in format 6:

- A dimension's `arcEndConditions`: where on a circle or arc each of its two ends is
  measured (`IDimension.GetArcEndCondition`, swArcEndCondition_e: 0 none, 1 centre, 2 min,
  3 max). A mate's `firstArcCondition` and `secondArcCondition` likewise for a distance mate.
- A sketch's `slots` (centre, centre point, slot points, length, width), and each relation's
  `entityTypes` (`GetEntitiesType`). SolidWorks gives the slots of a symmetric relation
  between two slots as null entities: where two slots' centres are mirror images about the
  relation's line, they are written in their place as `sketchSlot`s, with `resolved` saying so.
- A path mate's `pathSelections`: what editing the mate selects (its point and the path's
  edges or sketch segments, each with its component and selection mark). SolidWorks 2018
  has no `IPathMateFeatureData` (it is from 2019) and gives the path as an empty mate entity,
  so the mate is edited (Edit Feature), the selections read, and the edit cancelled. Skipped
  with `--no-selections`.
- A composite curve's `curve`: its edges (`IReferenceCurve.GetSegments`). Its definition is
  no longer read in an assembly, where it faults SolidWorks 2018 (RPC_E_SERVERFAULT).
- Assemblies, per configuration: `suppressedChildren`, the components of each suppressed
  sub-assembly (which SolidWorks doesn't list, though mates can name them), read from the
  sub-assembly's own document in the configuration the component shows, with `transform`
  (in this assembly, by the component's transform) and `ownTransform`;
  `massProperties` (volume, area, mass, centre of mass, SolidWorks' `box`, and `tightBox`
  from the bodies' extreme points); and with `--step`, a STEP file of the configuration
  (`<file>.<configuration>.step`), as for parts.

Added in format 7 (in-context features: SolidWorks' external references, "->" in the tree;
written for SolidWorks 2018 SP5's API and not yet run):

- `externalReferences` at the root of every document (`IModelDocExtension.ListExternalFileReferences`)
  and on each feature that has some (`IFeature.ListExternalFileReferences2`): one entry per
  reference with `model` (the assembly it was made in: its context), `component` (what it refers
  to, as SolidWorks names it), `feature`, `dataType` (the kind of reference: a sketch relation, a
  converted edge, an end condition, a plane, …), `status` and `statusName`
  (`swExternalReferenceStatus_e`: 0 broken, 1 locked, 3 in context, 4 out of context, 5 dangling),
  `entity` (SolidWorks' text for it), `featureComponent` (the in-context component),
  `configOption` and `configName`. A part exported alone has its in-context references out of
  context. An assembly's `InContextFeatHolder` features get theirs where SolidWorks gives any.
- Assemblies, per configuration: `inContext`, for each component with external references
  (`IComponent2.ListExternalFileReferencesCount`): its `component`, `file`, `configuration`,
  `transform`, `references` (`IComponent2.ListExternalFileReferences2`), and `features`: each of
  its part's features with references, read through the component (`IComponent2.FirstFeature`
  and sub-features), so that what they refer to is the assembly's. A sketch has `modelToSketch`
  and `relations`: each relation that reaches outside the sketch (an entity not among its own, or
  a definition entity), with `index` (its place in the sketch's list, as the part's own export
  has it), `type`, `dimension`, `entities` and `definitionEntities`. Each entity read there has
  its `component`, `componentFile` and `componentTransform`, and a sketch entity its
  `sketchTransform`. Another feature (an extrude up to another part's face, a plane on one) has
  its `definition` read with `AccessSelections(doc, component)`, so its selections are the
  other components' faces.
- A face, edge or vertex read in an assembly says its `component` (`IEntity.GetComponent`).

Added in format 8 (display states and appearances; written for SolidWorks 2018 SP5's API, its member names
checked against the 2018 interop assemblies, and not yet run):

- At the root, whether display states are linked to configurations, read both ways the API offers:
  `linkDisplayStatesToConfigurations` (`IConfigurationManager.LinkDisplayStatesToConfigurations`) and
  `linkedDisplayState` (`IModelDocExtension.LinkedDisplayState`).
- Per configuration, `appearance`: the document's own colour (`IModelDocExtension.GetMaterialPropertyValues`, this
  configuration). A colour is SolidWorks' MaterialPropertyValues: red, green, blue (0 to 1), ambient, diffuse,
  specular, shininess, transparency, emission.
- Per configuration, `displayStates` (parts and assemblies), in `IConfiguration.GetDisplayStates`' order, which
  gives the active one first (`active`). Each has `appearances` (`IModelDocExtension.GetRenderMaterials2` for that
  display state: each render material's `file`, `primaryColor` (a COLORREF), `rgb`, `transparency`, `colorForm`,
  `entities` it is on, `displayStates` it is linked to) and `properties` (`IConfiguration.GetDisplayStateProperties`).
  - In an assembly, `components`: for every component (as `components` lists them, at every level), `visible`
    (`IComponent2.GetVisibilityInAsmDisplayStates`: 0 hidden, 1 shown), `referencedDisplayState`
    (`GetReferencedDisplayStates`), `appearances` (`GetRenderMaterials2`), and from
    `IModelDocExtension.GetDisplayStateSetting` with the display state and the components: `displayMode`
    (swDisplayMode_e), `transparency` (swTransparencyState_e: 0 transparent, 1 not), `visibility`
    (swVisibilityState_e: 1 hidden, 2 shown) and `appearance` (`DisplayStateSpecMaterialPropertyValues`). All read by
    name, without changing anything. Then, as a check, each display state is applied (`IConfiguration.ApplyDisplayState`)
    and each component's `appliedVisible` (`IComponent2.Visible`) and `appliedAppearance` read; `applied` says
    whether it could be; the active one is applied again at the end. `componentVisibility` and
    `componentProperties` are what `IConfiguration.GetDisplayStateComponentVisibility` and
    `GetDisplayStateComponentProperties` (undocumented in 2018) give, as they come.
  - In a part, `bodies`, `features` and `faces`: what `IConfiguration.GetDisplayStateBodyProperties`,
    `GetDisplayStateFeatureProperties` and `GetDisplayStateFaceProperties` give (`entities` and `values`, as they come).
- Assemblies, each component also has its look in the active display state: `visible` (`IComponent2.Visible`),
  `hidden` (`IsHidden`, counting a hidden parent), `appearance` (`GetMaterialPropertyValues2`, this configuration),
  `modelAppearance` (its part's own, `GetModelMaterialPropertyValues`) and `referencedDisplayState`
  (`ReferencedDisplayState2`).
- Parts: a body's, a face's and a feature's own `appearance` where it has one (`HasMaterialPropertyValues`).

Added in format 9 (equations in force, and linear patterns up to a reference; run on SolidWorks 2018 SP5,
revision 26.5):

- Per configuration, each equation has `suppressed` (`IEquationMgr.Suppression`: suppressed in this configuration)
  and `configurationOption` (`GetConfigurationOption`, swInConfigurationOpts_e: 1 this configuration, 2 all, 3 the
  ones named), and the configuration has `disabledEquationCount` (`GetDisabledEquationCount`), `equationStatus`
  (`Status`) and `automaticSolveOrder`. `disabled` (`IEquationMgr.Disabled`) is still written, but don't go by
  it: SolidWorks 2018 gives true for every equation but the first, in every document, where
  `GetDisabledEquationCount` is 0 and SolidWorks applies them all (the jobs' parts have the values they give).
  An equation is in force unless it is suppressed in the configuration, or disabled where the count says some are
  (`tools/sw-import/equation-state.js`). An equation of one configuration only (`configurationOption` 1) has its
  own text in that configuration's list (Door Stiffener D's `"D4@Sketch12"`).
- A part's linear pattern has `editSelections`: what editing it (Edit Feature) selects, each with its `type`
  (swSelectType_e), `mark` and `entity`; the edit is cancelled. SolidWorks 2018 has the pattern's "up to
  reference" but no API for it (`D1EndCondition` and the rest are from 2019, and are listed in `unreadable`): the
  reference is the selection marked 0x200000 (2097152). Where it is also the second direction's edge it is listed
  once, with mark 2. Its offset is the pattern's dimension D5; which of the spacing (D3) and the count (D1) is
  kept is the one SolidWorks has driving (the other is driven, `drivenState` 1, with no equation on it). Which way
  the offset goes is not given. Read for every pattern not suppressed in the configuration, unless
  `--no-selections`.

Every read is guarded: what fails is logged and left out.

A configuration SolidWorks made for the flat pattern (`…SM-FLAT-PATTERN`) has its bodies
only: their box is the flat size.

Faces, edges and sketch entities that something refers to are written as objects with a
`kind`. A sketch entity is identified by its `id` (two numbers) and its sketch's name.

## Building

```powershell
dotnet build tools/sw-export/SwExport.csproj -c Release
```

The SolidWorks interop assemblies (`SolidWorks.Interop.sldworks.dll` and `swconst.dll`,
from `api\redist` in a SolidWorks installation) are looked for in the folder given by the
`SolidWorksInteropDir` environment variable or `-p:SolidWorksInteropDir=...`.

## Running it on a SolidWorks machine

SolidWorks cannot be started from an SSH session. `RunInDesktopSession.ps1` (copied
beside the executable) starts the exporter as a one-off scheduled task in the account's
logged-in desktop, waits for it and removes the task. The account needs an open desktop
session there, and the paths in the input list must be readable from it.

```powershell
scp tools/sw-export/bin/Release/net48/* USER@HOST:C:/Users/USER/AppData/Local/PeetSwExport/bin/
ssh USER@HOST powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File `
  C:\Users\USER\AppData\Local\PeetSwExport\bin\RunInDesktopSession.ps1 `
  -InputList C:\Users\USER\AppData\Local\PeetSwExport\inputs.txt `
  -OutputDirectory C:\Users\USER\AppData\Local\PeetSwExport\output -Step -Dxf
scp USER@HOST:C:/Users/USER/AppData/Local/PeetSwExport/output/* <local folder>
```

Door's interlocking flanges (suppressed in every configuration) are read with
`-UnsuppressAlwaysOff`, on Door alone, into a folder of its own (it writes a new
`Door.SLDPRT.swx.json`, which then replaces the one in `exports\v7-run`; `--step`/`--dxf`
as for that export):

```powershell
Set-Content C:\Users\USER\AppData\Local\PeetSwExport\door.txt '\\nas\Design\Solidworks\Templates\Door.SLDPRT'
ssh USER@HOST powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File `
  C:\Users\USER\AppData\Local\PeetSwExport\bin\RunInDesktopSession.ps1 `
  -InputList C:\Users\USER\AppData\Local\PeetSwExport\door.txt `
  -OutputDirectory C:\Users\USER\AppData\Local\PeetSwExport\door -Step -Dxf -UnsuppressAlwaysOff
```

`export.log` then has a line `<flange> in <configuration>: read unsuppressed (...)` for each
flange and configuration, and each flange's `Edges` in the file are no longer null.

## Status

Run against SolidWorks 2026 on the whole template library (201 parts, 29 assemblies):
all complete, in about 80 minutes, twice. The flat sizes in the DXF files agree with the
flat-pattern configurations' bodies and with the bend deduction by hand.

`formatVersion` is 9 (6 to 9 are described above). What each earlier version added:

- **2:** a definition has its `Is…` methods (`IsBossFeature` is the only thing that tells a
  boss-extrude from a cut-extrude; a part's first extrude is `IsBaseExtrude`) and its
  values per reference (a plane's `Constraint[0]`); a relation's entities say where they
  are, because the id a relation gives for a segment is not the id the sketch lists it
  under; and every face has a `point` that is on it.
- **3:** a sketch has its `blocks`: each instance with its definition's segments, in the
  definition's own coordinates, and `blockToSketch`, the transform that places them (the
  sketch's own list of segments leaves out what is inside a block). A dimension has
  `lineDirection`, the way it measures, in the model's coordinates: two points can be
  dimensioned along x, along y or straight across. And the selections of suppressed
  features are read where SolidWorks allows it, so that a suppressed flange has its
  edges and a suppressed pattern its direction.
- **4:** a library feature has `subFeatures`, the names of what is in it. The walk of the
  tree meets those first, as features of their own just above it (with their sketches
  and definitions), so they are written there and only named under it. A body has
  `tightBox`, from its extreme points: `box` (GetBodyBox) can be loose by a hundredth of
  a millimetre round a freeform face. A corner relief has `namedDimensions` if any of
  `D1` to `D6` answers by name.
- **5:** what the running SolidWorks has and the interop assemblies don't is read by name
  (`lateBound`, above): a linear pattern's "up to reference" (`D1EndCondition` 1, with
  `D1EndReference`, `D1EndRefOffset`, `D1EndRefReverseOffset`, `D1EndUseSeedReference`,
  `D1EndSeedReference`, `D1EndUseSpacing`), each instance's `GetTransform[i]`, a base
  flange's end conditions and relief, an edge flange's angle reference. A sketch text
  has its `format` (`typeFace`, `bold`, `italic`, `charHeight` in metres, `widthFactor`,
  `charSpacingFactor`, `escapement`, …). An object with no case of its own has its
  `type` and `values` in place of `"kind": "unknown"`. None of this is checked against
  SolidWorks yet: a value that can't be read is left out (or listed in `unreadable`),
  and never stops the document.

In the interop assemblies (SolidWorks 2018) a corner relief has no definition: there is
no data interface for one, only the calls that make one (`AddCornerReliefType`,
`AddCornerReliefCorner`, `FinishCornerRelief`). SolidWorks 2026's API has
`ICornerReliefFeatureData` (`CornerCount`, `CornerType`, `GetCornerAtIndex`), which the
fifth version reads by name if the running SolidWorks gives it. Its shape is also in the
faces it leaves in the body (each face says which feature made it). A
3D sketch's points and segments are in the model's coordinates (its `modelToSketch` is
the identity).

`tally.js` counts what a folder of exports uses: see
[docs/solidworks-library-tally.md](../../docs/solidworks-library-tally.md).

Still missing: the definitions of composite curves (33, in one assembly); what a mate
joins when its component is not loaded (the end is written with its numbers and no
entity); and the selections of some suppressed features, which SolidWorks will not
give.

Known behaviour:

- **No desktop session, no SolidWorks.** With the account logged off, the scheduled task
  never runs (result 267011, "has not run"); one that runs whether the account is logged
  on or not (S4U) gets no SolidWorks (`CO_E_SERVER_EXEC_FAILURE` after two minutes), and
  has no network credentials for the library's share anyway.

- **Flat patterns.** On a document opened read-only, SolidWorks writes the whole DXF,
  then faults and closes the document. The exporter takes the file if it is there
  (`flatDxfFault` records the fault) and opens the document again for the next
  configuration. Every entity is on layer 0, bend lines included.
- **A flat-pattern configuration is not always flat.** `unfolded` says whether it is;
  the Door's is not, so its flat size comes only from the DXF.
- **A crash.** If a document takes SolidWorks down, what was read is written and the
  rest of the list gets a new instance (five times at most).
