//! Smart components, as SolidWorks has them: a part that carries features it makes in the
//! parts it is placed against (a hinge's holes in a door, a vent hood's cut-out in a
//! wall). The part's smart features ([`peet_model::SmartFeature`], added with
//! `smart_feature`) are each a sketch of the part, drawn where the other part's face is
//! to be, and how deep to cut. In an assembly, `apply_smart_features` gives the component
//! the faces they go on (SolidWorks' references, in order) and makes each feature in the
//! part whose face it is: a sketch on that face whose entities are the smart sketch's,
//! converted in context (so they follow the component wherever its mates put it), and a
//! cut of it. FabSys's `ApplySmartFeatures` is this operation.

use std::path::PathBuf;
use std::sync::Arc;

use peet_document::Document;
use peet_model::in_context::{External, Geom, change_at, model_at};
use peet_model::{
    CompId, EndCondition, ExtrudeFeature, FeatureId, FeatureKind, MateEnd, MateGeom, Model,
    Operation, PlaneRef, SheetCutEnd, SheetCutFeature, SketchRef, SmartFeature,
};
use peet_sketch::Geometry;
use serde_json::{Map, Value, json};

use crate::args::Args;
use crate::assembly::CompSel;
use crate::in_context::{follow_entity, geometry, names_of, parse_path, resolve_path};
use crate::mate::MateEndSel;
use crate::op::Op;

/// The operations of this module: where they work, name, fields and what they do, for `help`.
pub(crate) const SMART_OPS: &[(&str, &str, &str, &str)] = &[(
    "an assembly",
    "apply_smart_features",
    "component (the smart component: a component's name or id, or a path [sub-assembly, ..., part] to one inside a sub-assembly), references (the faces its smart features go on, in order, each an end as a mate's: {component: name or path, face: selector})",
    "Apply a smart component's features (SolidWorks' smart components, FabSys's ApplySmartFeatures): each smart feature of its part becomes, in the part whose face it is given, a sketch on that face converted from the smart feature's sketch in context, so it follows the component, and a cut of it (a sheet metal cut in sheet metal). The reply's 'applied' lists each feature made.",
)];

/// A smart component operation.
#[derive(Clone, Debug, PartialEq)]
pub enum SmartOp {
    Apply {
        component: Vec<CompSel>,
        references: Vec<MateEndSel>,
    },
}

impl SmartOp {
    pub(crate) fn word(&self) -> &'static str {
        match self {
            Self::Apply { .. } => "apply_smart_features",
        }
    }
}

/// Reads the operation called `op`, if it is one of this module's.
pub(crate) fn parse(op: &str, a: &mut Args) -> Option<Result<Op, String>> {
    match op {
        "apply_smart_features" => Some((|| {
            let component = a.required("component", parse_path)?;
            let references = a.required("references", |v| match v {
                    Value::Array(list) => list.iter().map(MateEndSel::parse).collect(),
                    one @ Value::Object(_) => Ok(vec![MateEndSel::parse(one)?]),
                    other => Err(format!(
                        "expected a list of faces, each {{\"component\": ..., \"face\": selector}}, not {other}"
                    )),
                })?;
            Ok(Op::Smart(SmartOp::Apply {
                component,
                references,
            }))
        })()),
        _ => None,
    }
}

/// The smart features of a part, in its order, with their names: those not suppressed.
fn smart_features(part: &Model) -> Vec<(FeatureId, String, SmartFeature)> {
    part.features()
        .filter(|f| !f.suppressed)
        .filter_map(|f| match &f.kind {
            FeatureKind::Smart(s) => Some((f.id, f.name.clone(), (**s).clone())),
            _ => None,
        })
        .collect()
}

/// A name not yet used by a feature of `part`: `name`, or `name (2)`, `name (3)`, ...
fn free_name(part: &Model, name: &str) -> String {
    let taken = |n: &str| part.features().any(|f| f.name == n);
    if !taken(name) {
        return name.to_owned();
    }
    (2..)
        .map(|i| format!("{name} ({i})"))
        .find(|n| !taken(n))
        .unwrap_or_else(|| name.to_owned())
}

/// The name SolidWorks gives a smart feature applied in a part: `Smart-Feature1<HingeMale-1>`,
/// numbered on from those the part has.
fn applied_name(part: &Model, component: &str) -> String {
    let n = (1..)
        .find(|n| {
            let prefix = format!("Smart-Feature{n}<");
            !part.features().any(|f| f.name.starts_with(&prefix))
        })
        .unwrap_or(1);
    free_name(part, &format!("Smart-Feature{n}<{component}>"))
}

/// Whether the part is sheet metal (a cut in it is a sheet metal cut).
fn is_sheet_metal(part: &Model) -> bool {
    part.features()
        .any(|f| !f.suppressed && f.kind.is_sheet_metal())
}

/// The cut a smart feature makes in `part`, of `sketch`.
fn cut_kind(part: &Model, sketch: FeatureId, s: &SmartFeature) -> FeatureKind {
    if is_sheet_metal(part) {
        return FeatureKind::SheetCut(Box::new(SheetCutFeature {
            sketch,
            end: s.end,
            depth: s.depth.clone(),
            reverse: s.reverse,
            both: false,
        }));
    }
    let mut params = peet_model::Extrude::new(Operation::Cut);
    params.end = match s.end {
        // Through the material the face is on: SolidWorks' "up to next".
        SheetCutEnd::Sheet => EndCondition::ThroughNext,
        SheetCutEnd::Blind => EndCondition::Blind,
        SheetCutEnd::ThroughAll => EndCondition::ThroughAll,
    };
    params.depth = s.depth.clone();
    params.reverse = s.reverse;
    FeatureKind::Extrude(Box::new(ExtrudeFeature { sketch, params }))
}

/// One smart feature made in a part, for the reply.
struct Applied {
    part: Vec<String>,
    path: Vec<CompId>,
    feature: String,
    sketch: String,
    smart_feature: String,
    reference: u32,
    entities: usize,
}

/// `apply_smart_features`. Returns the undo label.
pub(crate) fn apply(
    doc: &Document,
    model: &mut Model,
    data: &mut Map<String, Value>,
    component: &[CompSel],
    references: &[MateEndSel],
) -> Result<String, String> {
    let assembly = doc
        .model
        .assembly()
        .ok_or_else(|| format!("{} is a part: it has no components.", doc.title()))?;
    let (own, own_names) = resolve_path(doc, component)?;
    let name = own_names.join(" / ");
    let smart_part = model_at(&doc.model, &own).ok_or("The component's part is gone.")?;
    if smart_part.is_assembly() {
        return Err(format!(
            "{name} is a sub-assembly: give the path down to the smart part in it, as [\"{name}\", \"Part-1\"]."
        ));
    }
    let smart = smart_features(&smart_part);
    if smart.is_empty() {
        return Err(format!(
            "{name} is not a smart component: its part, {}, has no smart features (add them to the part with 'smart_feature').",
            smart_part.name
        ));
    }
    let needed = smart
        .iter()
        .map(|(_, _, s)| s.reference + 1)
        .max()
        .unwrap_or(0) as usize;
    if references.len() < needed {
        return Err(format!(
            "{name}'s smart features go on {needed} face{}, and {} {} given: give one for each, in order ({}).",
            if needed == 1 { "" } else { "s" },
            references.len(),
            if references.len() == 1 { "was" } else { "were" },
            smart
                .iter()
                .map(|(_, n, s)| format!("{n} on face {}", s.reference + 1))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let targets: Vec<MateEnd> = references
        .iter()
        .map(|r| r.resolve(doc))
        .collect::<Result<_, _>>()?;
    for (i, t) in targets.iter().enumerate() {
        if t.path.is_empty() || !matches!(t.geom, Some(MateGeom::Face(_))) {
            return Err(format!(
                "Reference {} is not a face of a component: give {{\"component\": ..., \"face\": selector}}.",
                i + 1
            ));
        }
        if t.path == own {
            return Err(format!(
                "Reference {} is a face of {name} itself: give the face of the part its features go into.",
                i + 1
            ));
        }
    }
    let context = doc.title();
    let label = format!("Apply Smart Features {name}");
    let mut applied = Vec::new();
    for (_, smart_name, s) in &smart {
        let sketch_name = smart_part.name_of(s.sketch).to_owned();
        // The smart sketch's curves, converted onto the face where the component puts
        // them.
        let curves: Vec<_> = smart_part
            .feature(s.sketch)
            .and_then(|f| f.sketch())
            .map(|sk| {
                sk.sketch
                    .entities()
                    .filter(|(_, e)| {
                        !e.construction && !matches!(e.geometry, Geometry::Point { .. })
                    })
                    .map(|(id, _)| id)
                    .collect()
            })
            .unwrap_or_default();
        if curves.is_empty() {
            return Err(format!(
                "Smart feature {smart_name} of {} has nothing to cut: its sketch {sketch_name} has no lines, arcs or circles.",
                smart_part.name
            ));
        }
        let reference = choose(doc, &own, s, &curves, &targets)?;
        let target = &targets[reference];
        let Some(MateGeom::Face(face)) = &target.geom else {
            continue;
        };
        let target_names = names_of(assembly, &target.path);
        let part_name = target_names.join(" / ");
        // The face's plane, in its part's coordinates.
        let Geom::Plane(plane) = geometry(doc, &target.path, target)? else {
            return Err(format!(
                "Reference {} ({part_name}) is not a flat face: a smart feature is cut from a flat face.",
                reference + 1
            ));
        };
        let mut part = (*model_at(model, &target.path).ok_or("The part is gone.")?).clone();
        if part.is_assembly() {
            return Err(format!(
                "{part_name} is a sub-assembly: give a face of a part in it."
            ));
        }
        let sketch = part.add_sketch(PlaneRef::Face(face.clone()), plane);
        for entity in &curves {
            let from = MateEnd {
                path: own.clone(),
                geom: Some(MateGeom::Sketch(SketchRef {
                    feature: s.sketch,
                    entity: *entity,
                })),
            };
            let last = geometry(doc, &target.path, &from)?;
            let reference = |last: Geom| External {
                context: context.clone(),
                component: target.path.clone(),
                target: from.clone(),
                component_names: target_names.clone(),
                target_names: own_names.clone(),
                last,
                locked: false,
                broken: false,
            };
            follow_entity(&mut part, sketch, None, false, last, reference, &part_name)
                .map_err(|e| format!("Smart feature {smart_name} on {part_name}: {e}"))?;
        }
        let made_name = applied_name(&part, own_names.last().map_or("", String::as_str));
        let sketch_label = free_name(&part, &format!("{made_name} {sketch_name}"));
        if let Some(f) = part.feature_mut(sketch) {
            f.name = sketch_label.clone();
            f.visible = false;
        }
        let cut = part.add(cut_kind(&part, sketch, s));
        if let Some(f) = part.feature_mut(cut) {
            f.name = made_name.clone();
        }
        let (changed, ()) = change_at(model, &target.path, &mut |m| *m = part.clone())
            .ok_or("The part is gone.")?;
        *model = changed;
        applied.push(Applied {
            part: target_names,
            path: target.path.clone(),
            feature: made_name,
            sketch: sketch_label,
            smart_feature: smart_name.clone(),
            reference: reference as u32,
            entities: curves.len(),
        });
    }
    // A linked part (at any depth) is written, as `in_context` writes one.
    let mut written: Vec<PathBuf> = Vec::new();
    for a in &applied {
        if let Some(file) = write_deepest_link(doc, model, &a.path)?
            && !written.contains(&file)
        {
            written.push(file);
        }
    }
    data.insert(
        "applied".to_owned(),
        json!(
            applied
                .iter()
                .map(|a| json!({
                    "part": a.part.join(" / "),
                    "feature": a.feature,
                    "sketch": a.sketch,
                    "smart_feature": a.smart_feature,
                    "reference": a.reference,
                    "entities": a.entities,
                }))
                .collect::<Vec<_>>()
        ),
    );
    if !written.is_empty() {
        data.insert(
            "written".to_owned(),
            json!(
                written
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
            ),
        );
    }
    Ok(label)
}

/// Which of the faces a smart feature goes on: the face, parallel to its sketch, that the
/// component puts it on (nearest the sketch's plane, then nearest the sketch in the
/// face), as SolidWorks matched it in the assembly the smart component was made in.
/// None parallel to it: its `reference`, the order SolidWorks would take them in.
fn choose(
    doc: &Document,
    own: &[CompId],
    s: &SmartFeature,
    curves: &[peet_sketch::EntityId],
    targets: &[MateEnd],
) -> Result<usize, String> {
    let Some(first) = curves.first() else {
        return Ok(s.reference as usize);
    };
    let from = MateEnd {
        path: own.to_vec(),
        geom: Some(MateGeom::Sketch(SketchRef {
            feature: s.sketch,
            entity: *first,
        })),
    };
    let mut best: Option<(f64, f64, usize)> = None;
    for (i, t) in targets.iter().enumerate() {
        let (Ok(Geom::Plane(plane)), Ok(at)) =
            (geometry(doc, &t.path, t), geometry(doc, &t.path, &from))
        else {
            continue;
        };
        let (p, axis) = match at {
            Geom::Circle { center, normal, .. } | Geom::Arc { center, normal, .. } => {
                (center, Some(normal))
            }
            Geom::Line(a, b) => (a.lerp(b, 0.5), None),
            Geom::Point(p) => (p, None),
            Geom::Plane(_) => continue,
        };
        let n = plane.frame.z_axis();
        if axis.is_some_and(|a| a.cross(n).length() > 1e-6) {
            continue;
        }
        let off = (p - plane.origin()).dot(n).abs();
        let centre = face_centre(doc, &t.path, t).unwrap_or(plane.origin());
        let across = ((p - centre) - n * (p - centre).dot(n)).length();
        // To a hundredth of a millimetre: faces in one plane are told apart by where they are
        let off = (off * 100.0).round();
        let better = match best {
            None => true,
            Some((o, a, _)) => off < o || (off == o && across < a - 1e-6),
        };
        if better {
            best = Some((off, across, i));
        }
    }
    // None parallel to it: as SolidWorks, by order
    Ok(best.map_or(s.reference as usize, |(_, _, i)| i))
}

/// The middle of a face given as a mate end, in its part's coordinates.
fn face_centre(doc: &Document, path: &[CompId], end: &MateEnd) -> Option<peet_math::DVec3> {
    let Some(MateGeom::Face(face)) = &end.geom else {
        return None;
    };
    let part = model_at(&doc.model, path)?;
    let built = Document::from_model((*part).clone(), None);
    let bodies = &built.evaluation().bodies;
    let found = peet_model::naming::find_face(bodies, face)?;
    Some(bodies[found.body].face_center(found.id))
}

/// Writes the file of the deepest linked part or sub-assembly along `path` (the one
/// holding what changed), if there is one. Returns the file written.
fn write_deepest_link(
    doc: &Document,
    model: &Model,
    path: &[CompId],
) -> Result<Option<PathBuf>, String> {
    let mut at: Arc<Model> = Arc::new(model.clone());
    let mut deepest = None;
    for id in path {
        let Some(a) = at.assembly() else { break };
        let Some(d) = a.definition_of(*id) else { break };
        if let Some(link) = &d.link {
            deepest = Some((link.clone(), d.model.clone()));
        }
        at = d.model.clone();
    }
    let Some((link, part)) = deepest else {
        return Ok(None);
    };
    crate::links::unavailable()?;
    let file = crate::links::find(&link, crate::links::folder(doc).as_deref())
        .unwrap_or_else(|| PathBuf::from(&link.path));
    let bytes = peet_io::document::save(
        &part.for_file(file.parent()),
        &peet_io::document::Metadata::default(),
        None,
    )
    .map_err(|e| e.message)?;
    peet_platform::write_file(&file, &bytes)?;
    Ok(Some(file))
}
