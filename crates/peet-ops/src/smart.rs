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
    AxisRef, CompId, EndCondition, ExtrudeFeature, FeatureId, FeatureKind, LinearDirection,
    MateEnd, MateGeom, Model, Operation, Output, PatternDef, PatternFeature, PlaneRef, Scalar,
    ScalarKind, SheetCutEnd, SheetCutFeature, SketchRef, SmartFeature, StdAxis, smart_seeds,
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
    /// The patterns of the cut, and how many of it each makes (the original included).
    patterns: Vec<(String, u32)>,
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
    let mut left_out = Vec::new();
    for (smart_id, smart_name, s) in &smart {
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
        // The face given in its place among the references: SolidWorks takes them in its
        // library features' order (job 21441-1: a Plinth bolt's first feature, made where the
        // plinth was, went into the Floor, its first face)
        let reference = s.reference as usize;
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
            // Seen flat on the face, or the face is not the one the component sits against
            let n = plane.frame.z_axis();
            let square = match last {
                Geom::Circle { normal, .. } | Geom::Arc { normal, .. } => {
                    normal.cross(n).length() < 1e-6
                }
                Geom::Line(a, b) => (b - a).normalize_or_zero().dot(n).abs() < 1e-6,
                _ => true,
            };
            if !square {
                return Err(format!(
                    "Smart feature {smart_name} of {name} is not parallel to reference {} ({part_name}): give the face of {part_name} that {name} sits against.",
                    reference + 1
                ));
            }
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
        // Its patterns in the smart part, as many as they come to there now
        let mut patterns = Vec::new();
        let made = smart_patterns(doc, &own, &target.path, &smart_part, *smart_id)
            .map_err(|e| format!("Smart feature {smart_name} of {name}: {e}"))?;
        for (pattern_name, def, copies) in made {
            let id = part.add(FeatureKind::Pattern(Box::new(PatternFeature {
                seeds: vec![cut],
                def,
            })));
            let label = free_name(&part, &format!("{made_name} {pattern_name}"));
            if let Some(f) = part.feature_mut(id) {
                f.name = label.clone();
            }
            patterns.push((label, copies));
        }
        // A feature whose cut takes nothing off the part (its sketch is off the face given)
        // is left out, as SolidWorks leaves it out: Esc Surround Mid's on Esc Surround L and
        // R in job 21441-1, each drawn over the other
        if let Some(why) = cuts_nothing(&part, cut) {
            left_out.push(json!({
                "smart_feature": smart_name,
                "part": target_names.join(" / "),
                "reference": reference,
                "why": why,
            }));
            continue;
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
            patterns,
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
                    "patterns": a
                        .patterns
                        .iter()
                        .map(|(n, c)| json!({ "feature": n, "count": c }))
                        .collect::<Vec<_>>(),
                }))
                .collect::<Vec<_>>()
        ),
    );
    if !left_out.is_empty() {
        data.insert("left_out".to_owned(), json!(left_out));
    }
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

/// The linear patterns of smart feature `seed` in its part (those that copy smart features
/// only), as they are made in the part it is applied in: SolidWorks works out a library
/// feature's equations against the smart part as it is when it is applied (FabSys sizes the
/// part first), so each count and spacing is its value in the smart part now, and each
/// direction the one it comes to in the other part (`target`), which must be along one of
/// that part's axes. A direction of one (the seed alone, as an equation can make it) copies
/// nothing. Each with its name in the smart part and how many it makes.
fn smart_patterns(
    doc: &Document,
    own: &[CompId],
    target: &[CompId],
    smart_part: &Model,
    seed: FeatureId,
) -> Result<Vec<(String, PatternDef, u32)>, String> {
    let patterns: Vec<_> = smart_part
        .features()
        .filter(|f| !f.suppressed)
        .filter_map(|f| match &f.kind {
            FeatureKind::Pattern(p)
                if p.seeds.contains(&seed) && smart_seeds(smart_part, &p.seeds) =>
            {
                Some((f.name.clone(), p.def.clone()))
            }
            _ => None,
        })
        .collect();
    if patterns.is_empty() {
        return Ok(Vec::new());
    }
    let mut sized = smart_part.clone();
    sized.refresh_dimension_values();
    let params = &sized.parameters;
    let eval = doc.evaluation();
    let (Some(there), Some(here)) = (eval.component_frame(target), eval.component_frame(own))
    else {
        return Err("a component it involves is suppressed or not placed.".to_owned());
    };
    let turn = there.inverse().compose(&here).rotation;
    let built = Document::from_model(smart_part.clone(), None);
    let mut out = Vec::new();
    for (pattern_name, def) in patterns {
        let PatternDef::Linear { first, second } = def else {
            return Err(format!(
                "its pattern {pattern_name} is circular: only linear patterns of smart features are made where it is applied."
            ));
        };
        let direction = |d: &LinearDirection| -> Result<(LinearDirection, u32), String> {
            if d.up_to.is_some() {
                return Err(format!(
                    "its pattern {pattern_name} runs up to a reference: give it a count and a spacing."
                ));
            }
            let along = match &d.direction {
                AxisRef::Standard(a) => a.axis().dir,
                AxisRef::Feature(id) => match built.evaluation().output(*id) {
                    Output::Axis(a) => a.dir,
                    Output::Frame(f) => f.z_axis(),
                    _ => {
                        return Err(format!(
                            "its pattern {pattern_name}'s direction is not built."
                        ));
                    }
                },
                AxisRef::Edge(_) => {
                    return Err(format!(
                        "its pattern {pattern_name} runs along an edge: give it a standard axis or a reference axis."
                    ));
                }
            };
            let along = turn * if d.flip { -along } else { along };
            let Some((axis, flip)) = StdAxis::ALL.iter().find_map(|a| {
                let k = a.axis().dir.dot(along);
                (k.abs() > 1.0 - 1e-6).then_some((*a, k < 0.0))
            }) else {
                return Err(format!(
                    "its pattern {pattern_name} runs at an angle to the axes of the part it is applied in."
                ));
            };
            let mut count = d
                .count
                .evaluate(ScalarKind::Number, params)
                .map_err(|m| format!("its pattern {pattern_name}'s count: {m}."))?;
            if d.count.expression.is_some() {
                count = count.round();
            }
            let spacing = d
                .spacing
                .evaluate(ScalarKind::Length, params)
                .map_err(|m| format!("its pattern {pattern_name}'s spacing: {m}."))?;
            let count = count.clamp(1.0, 10_000.0) as u32;
            Ok((
                LinearDirection {
                    direction: AxisRef::Standard(axis),
                    spacing: Scalar::new(spacing),
                    count: Scalar::new(f64::from(count)),
                    flip,
                    up_to: None,
                },
                count,
            ))
        };
        let mut rows: Vec<_> = std::iter::once(&first)
            .chain(second.as_ref())
            .map(direction)
            .collect::<Result<_, _>>()?;
        rows.retain(|(_, n)| *n > 1);
        let copies = rows.iter().map(|(_, n)| n).product();
        let mut rows = rows.into_iter().map(|(d, _)| d);
        if let Some(first) = rows.next() {
            out.push((
                pattern_name,
                PatternDef::Linear {
                    first,
                    second: rows.next(),
                },
                copies,
            ));
        }
    }
    Ok(out)
}

/// Why the cut `cut` of `part` takes nothing off it, if it doesn't: the part built as it is
/// (its in-context sketch where the assembly puts it now).
fn cuts_nothing(part: &Model, cut: FeatureId) -> Option<String> {
    let built = Document::from_model(part.clone(), None);
    let message = built.evaluation().status(cut)?.message()?.to_owned();
    (message.contains("doesn't remove any material") || message.contains("nothing in its way"))
        .then_some(message)
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
