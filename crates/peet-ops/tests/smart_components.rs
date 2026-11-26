//! Smart components, as SolidWorks has them: a part (a hinge) whose smart features (its
//! holes) are cut, in an assembly, into the part it is placed against (a door), where the
//! component is, following it when it moves.

use peet_document::Document;
use peet_ops::{Undo, apply_json};
use serde_json::{Value, json};

fn ok(doc: &mut Document, op: Value) -> Value {
    let reply = apply_json(doc, &op, Undo::Step);
    assert!(reply.ok, "{op} failed: {}", reply.json["error"]);
    reply.json
}

fn error(doc: &mut Document, op: Value) -> String {
    let before = doc.model.clone();
    let reply = apply_json(doc, &op, Undo::Step);
    assert!(!reply.ok, "{op} should have failed: {}", reply.json);
    assert_eq!(doc.model, before, "a failed operation changes nothing");
    reply.json["error"].as_str().unwrap().to_owned()
}

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("peet-smart-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A door: a 2 mm sheet 300 × 200 on the top plane. A hinge: a block 40 × 20 × 10 whose
/// smart feature is two 6 mm holes in the face it sits on (its bottom, z = 0).
fn parts(dir: &std::path::Path) -> (String, String) {
    let door = dir.join("Door.peet").to_string_lossy().into_owned();
    let hinge = dir.join("Hinge.peet").to_string_lossy().into_owned();
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [300, 200]}]}),
    );
    ok(
        &mut doc,
        json!({"op": "base_flange", "sketch": "Sketch1", "thickness": 2}),
    );
    ok(&mut doc, json!({"op": "save", "path": door}));

    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [40, 20]}]}),
    );
    ok(
        &mut doc,
        json!({"op": "extrude", "sketch": "Sketch1", "depth": 10}),
    );
    ok(
        &mut doc,
        json!({"op": "sketch", "on": "top", "name": "Hinge Holes", "draw": [
            {"type": "circle", "center": [10, 10], "radius": 3},
            {"type": "circle", "center": [30, 10], "radius": 3}]}),
    );
    let made = ok(
        &mut doc,
        json!({"op": "smart_feature", "name": "HingeHoles", "sketch": "Hinge Holes"}),
    );
    assert_eq!(made["created"][0]["name"], "HingeHoles", "{made}");
    // The hinge itself is not cut: a smart feature is for the part it is placed on.
    let bodies = ok(&mut doc, json!({"op": "bodies"}));
    assert!(
        (bodies["bodies"][0]["volume_mm3"].as_f64().unwrap() - 8000.0).abs() < 1e-6,
        "{bodies}"
    );
    ok(&mut doc, json!({"op": "save", "path": hinge}));
    (door, hinge)
}

fn door_volume(doc: &Document) -> f64 {
    let all = doc.model.assembly().unwrap();
    let door = all.components().find(|c| c.name == "Door-1").unwrap();
    let part = all.definition_of(door.id).unwrap().model.clone();
    let mut alone = Document::from_model((*part).clone(), None);
    let bodies = ok(&mut alone, json!({"op": "bodies"}));
    bodies["bodies"][0]["volume_mm3"].as_f64().unwrap()
}

fn hole_centres(doc: &mut Document) -> Vec<[f64; 2]> {
    let listed = ok(
        doc,
        json!({"op": "external_references", "component": "Door-1"}),
    );
    let mut out: Vec<[f64; 2]> = listed["references"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let at = &r["at"];
            [at[0].as_f64().unwrap(), at[1].as_f64().unwrap()]
        })
        .collect();
    out.sort_by(|a, b| a.partial_cmp(b).unwrap());
    out
}

fn assembly(link: bool) -> (Document, std::path::PathBuf) {
    let dir = temp(if link { "linked" } else { "own" });
    let (door, hinge) = parts(&dir);
    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "new", "assembly": true}));
    ok(
        &mut doc,
        json!({"op": "insert", "path": door, "name": "Door-1", "fixed": true, "link": link}),
    );
    ok(
        &mut doc,
        json!({"op": "insert", "path": hinge, "name": "Hinge-1", "at": [50, 60, 2], "link": link}),
    );
    if link {
        ok(
            &mut doc,
            json!({"op": "save", "path": dir.join("Frame.peet").to_string_lossy()}),
        );
    }
    (doc, dir)
}

const HOLE: f64 = std::f64::consts::PI * 9.0 * 2.0;

#[test]
fn a_hinge_cuts_its_holes_in_the_door_and_they_follow_it() {
    let (mut doc, _) = assembly(false);
    let before = door_volume(&doc);
    assert!((before - 120_000.0).abs() < 1e-6);
    let applied = ok(
        &mut doc,
        json!({"op": "apply_smart_features", "component": "Hinge-1",
            "references": [{"component": "Door-1", "face": {"normal": [0, 0, 1]}}]}),
    );
    let made = &applied["applied"][0];
    assert_eq!(made["part"], "Door-1", "{applied}");
    assert_eq!(made["feature"], "Smart-Feature1<Hinge-1>", "{applied}");
    assert_eq!(made["entities"], 2, "{applied}");
    assert!(
        (door_volume(&doc) - (before - 2.0 * HOLE)).abs() < 1e-3,
        "two 6 mm holes through the 2 mm door: {}",
        door_volume(&doc)
    );
    assert_eq!(hole_centres(&mut doc), [[60.0, 70.0], [80.0, 70.0]]);

    // The hinge moves: its holes in the door go with it.
    ok(
        &mut doc,
        json!({"op": "place", "component": "Hinge-1", "at": [150, 100, 2]}),
    );
    assert_eq!(hole_centres(&mut doc), [[160.0, 110.0], [180.0, 110.0]]);
    assert!((door_volume(&doc) - (before - 2.0 * HOLE)).abs() < 1e-3);

    // Undone in one step.
    assert!(doc.undo().is_some());
    assert!(doc.undo().is_some());
    assert!((door_volume(&doc) - before).abs() < 1e-6);
}

#[test]
fn applied_to_a_linked_door_its_file_has_the_holes() {
    let (mut doc, dir) = assembly(true);
    let applied = ok(
        &mut doc,
        json!({"op": "apply_smart_features", "component": "Hinge-1",
            "references": [{"component": "Door-1", "face": {"normal": [0, 0, 1]}}]}),
    );
    assert!(
        applied["written"][0]
            .as_str()
            .unwrap()
            .ends_with("Door.peet"),
        "{applied}"
    );
    let mut door = Document::default();
    ok(
        &mut door,
        json!({"op": "open", "path": dir.join("Door.peet").to_string_lossy()}),
    );
    let bodies = ok(&mut door, json!({"op": "bodies"}));
    let volume = bodies["bodies"][0]["volume_mm3"].as_f64().unwrap();
    assert!((volume - (120_000.0 - 2.0 * HOLE)).abs() < 1e-3, "{bodies}");
}

#[test]
fn what_is_not_a_smart_component_or_given_too_few_faces_is_refused() {
    let (mut doc, _) = assembly(false);
    let e = error(
        &mut doc,
        json!({"op": "apply_smart_features", "component": "Door-1",
            "references": [{"component": "Hinge-1", "face": {"normal": [0, 0, 1]}}]}),
    );
    assert!(e.contains("not a smart component"), "{e}");
    let e = error(
        &mut doc,
        json!({"op": "apply_smart_features", "component": "Hinge-1", "references": []}),
    );
    assert!(e.contains("go on 1 face"), "{e}");
    let e = error(
        &mut doc,
        json!({"op": "apply_smart_features", "component": "Hinge-1",
            "references": [{"component": "Hinge-1", "face": {"normal": [0, 0, -1]}}]}),
    );
    assert!(e.contains("itself"), "{e}");
    let e = error(
        &mut doc,
        json!({"op": "apply_smart_features", "component": "Hinge-1",
            "references": [{"component": "Door-1", "face": {"normal": [1, 0, 0]}}]}),
    );
    assert!(e.contains("not parallel"), "{e}");
}

#[test]
fn each_smart_feature_goes_on_the_face_given_in_its_place() {
    // Each smart feature goes on the face given in its place among the references, as
    // SolidWorks takes them, wherever its sketch is (job 21441-1: a Plinth bolt's first
    // feature, drawn where the plinth was, went into the Floor, given first). One whose cut
    // would take nothing off its part is left out, as SolidWorks leaves it out (Esc Surround
    // Mid's, each drawn over the other surround).
    let dir = temp("two");
    let plate = dir.join("Plate.peet").to_string_lossy().into_owned();
    let bolt = dir.join("Bolt.peet").to_string_lossy().into_owned();
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [300, 200]}]}),
    );
    ok(
        &mut doc,
        json!({"op": "extrude", "sketch": "Sketch1", "depth": 2}),
    );
    ok(&mut doc, json!({"op": "save", "path": plate}));
    // Each plate a part of its own
    let other = dir.join("Other.peet").to_string_lossy().into_owned();
    ok(&mut doc, json!({"op": "save", "path": other}));
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [-10, -10], "to": [10, 10]}]}),
    );
    ok(
        &mut doc,
        json!({"op": "extrude", "sketch": "Sketch1", "depth": 10}),
    );
    ok(
        &mut doc,
        json!({"op": "sketch", "on": "top", "name": "Near hole", "draw": [{"type": "circle", "center": [0, 0], "radius": 3}]}),
    );
    ok(
        &mut doc,
        json!({"op": "sketch", "on": "top", "name": "Far hole", "draw": [{"type": "circle", "center": [400, 0], "radius": 3}]}),
    );
    ok(
        &mut doc,
        json!({"op": "smart_feature", "sketch": "Near hole", "reference": 0}),
    );
    ok(
        &mut doc,
        json!({"op": "smart_feature", "sketch": "Far hole", "reference": 1}),
    );
    ok(&mut doc, json!({"op": "save", "path": bolt}));
    // Two plates side by side, under the near hole and the far one
    let apply = |first: &str, second: &str| {
        let mut doc = Document::default();
        ok(&mut doc, json!({"op": "new", "assembly": true}));
        ok(
            &mut doc,
            json!({"op": "insert", "path": plate, "name": "Near-1", "fixed": true, "at": [0, 0, -2]}),
        );
        ok(
            &mut doc,
            json!({"op": "insert", "path": other, "name": "Far-1", "fixed": true, "at": [400, 0, -2]}),
        );
        ok(
            &mut doc,
            json!({"op": "insert", "path": bolt, "name": "Bolt-1", "fixed": true, "at": [50, 50, 0]}),
        );
        ok(
            &mut doc,
            json!({"op": "apply_smart_features", "component": "Bolt-1", "references": [
                {"component": first, "face": {"normal": [0, 0, 1]}},
                {"component": second, "face": {"normal": [0, 0, 1]}}]}),
        )
    };
    let parts = |reply: &Value, key: &str| -> Vec<(String, u64)> {
        reply[key]
            .as_array()
            .map(|list| {
                list.iter()
                    .map(|a| {
                        (
                            a["part"].as_str().unwrap().to_owned(),
                            a["reference"].as_u64().unwrap(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let applied = apply("Near-1", "Far-1");
    assert_eq!(
        parts(&applied, "applied"),
        [("Near-1".to_owned(), 0), ("Far-1".to_owned(), 1)],
        "{applied}"
    );
    assert!(applied.get("left_out").is_none(), "{applied}");
    // Given the other way round, each goes on the other plate, where it cuts nothing
    let swapped = apply("Far-1", "Near-1");
    assert!(parts(&swapped, "applied").is_empty(), "{swapped}");
    assert_eq!(
        parts(&swapped, "left_out"),
        [("Far-1".to_owned(), 0), ("Near-1".to_owned(), 1)],
        "{swapped}"
    );
}

#[test]
fn a_hinge_in_a_sub_assembly_cuts_a_door_in_another() {
    // As FabSys has them: HingeMale-1@Hinge Assembly-1@Door Assembly-1, applied in the
    // enclosure to the door in the door assembly. Every file is linked.
    let dir = temp("nested");
    let (door, hinge) = parts(&dir);
    let file = |n: &str| dir.join(n).to_string_lossy().into_owned();
    let mut sub = Document::default();
    ok(&mut sub, json!({"op": "new", "assembly": true}));
    ok(
        &mut sub,
        json!({"op": "insert", "path": hinge, "name": "Hinge-1", "fixed": true, "link": true}),
    );
    ok(
        &mut sub,
        json!({"op": "save", "path": file("Hinge Assembly.peet")}),
    );
    let mut door_assembly = Document::default();
    ok(&mut door_assembly, json!({"op": "new", "assembly": true}));
    ok(
        &mut door_assembly,
        json!({"op": "insert", "path": door, "name": "Door-1", "fixed": true, "link": true}),
    );
    ok(
        &mut door_assembly,
        json!({"op": "insert", "path": file("Hinge Assembly.peet"), "name": "Hinge Assembly-1",
            "at": [50, 60, 2], "fixed": true, "link": true}),
    );
    ok(
        &mut door_assembly,
        json!({"op": "save", "path": file("Door Assembly.peet")}),
    );
    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "new", "assembly": true}));
    ok(
        &mut doc,
        json!({"op": "insert", "path": file("Door Assembly.peet"), "name": "Door Assembly-1",
            "fixed": true, "link": true}),
    );
    ok(
        &mut doc,
        json!({"op": "save", "path": file("Enclosure.peet")}),
    );
    let applied = ok(
        &mut doc,
        json!({"op": "apply_smart_features",
            "component": ["Door Assembly-1", "Hinge Assembly-1", "Hinge-1"],
            "references": [{"component": ["Door Assembly-1", "Door-1"], "face": {"normal": [0, 0, 1]}}]}),
    );
    assert_eq!(
        applied["applied"][0]["part"], "Door Assembly-1 / Door-1",
        "{applied}"
    );
    assert!(
        applied["written"][0]
            .as_str()
            .unwrap()
            .ends_with("Door.peet"),
        "{applied}"
    );
    ok(&mut doc, json!({"op": "save"}));
    // The door's file has the holes, where the hinge is.
    let mut alone = Document::default();
    ok(&mut alone, json!({"op": "open", "path": file("Door.peet")}));
    let bodies = ok(&mut alone, json!({"op": "bodies"}));
    let volume = bodies["bodies"][0]["volume_mm3"].as_f64().unwrap();
    assert!((volume - (120_000.0 - 2.0 * HOLE)).abs() < 1e-3, "{bodies}");
    // The enclosure, opened again, has them too, in context.
    let mut again = Document::default();
    ok(
        &mut again,
        json!({"op": "open", "path": file("Enclosure.peet")}),
    );
    let listed = ok(&mut again, json!({"op": "external_references"}));
    let statuses: Vec<&str> = listed["references"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["status"].as_str().unwrap())
        .collect();
    assert_eq!(statuses, ["in_context", "in_context"], "{listed}");
}

/// A hinge whose hole is a smart feature patterned along x, as many as its `Holes`
/// parameter says (SolidWorks' library feature patterns, their count from an equation).
fn patterned_hinge(dir: &std::path::Path, holes: u32) -> String {
    let hinge = dir
        .join(format!("Hinge{holes}.peet"))
        .to_string_lossy()
        .into_owned();
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op": "set_parameter", "name": "Holes", "value": "2"}),
    );
    ok(
        &mut doc,
        json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [140, 20]}]}),
    );
    ok(
        &mut doc,
        json!({"op": "extrude", "sketch": "Sketch1", "depth": 10}),
    );
    ok(
        &mut doc,
        json!({"op": "sketch", "on": "top", "name": "Hinge Holes", "draw": [
            {"type": "circle", "center": [10, 10], "radius": 3}]}),
    );
    ok(
        &mut doc,
        json!({"op": "smart_feature", "name": "HingeHoles", "sketch": "Hinge Holes"}),
    );
    ok(
        &mut doc,
        json!({"op": "linear_pattern", "name": "LPattern1", "features": ["HingeHoles"], "direction": "x", "spacing": 40, "count": "Holes"}),
    );
    // The pattern changes nothing in the hinge itself, as its smart feature doesn't
    let bodies = ok(&mut doc, json!({"op": "bodies"}));
    assert!(
        (bodies["bodies"][0]["volume_mm3"].as_f64().unwrap() - 28_000.0).abs() < 1e-6,
        "{bodies}"
    );
    // Sized before it is applied, as FabSys sizes a template
    ok(
        &mut doc,
        json!({"op": "set_parameter", "name": "Holes", "value": holes.to_string()}),
    );
    let status = ok(&mut doc, json!({"op": "status"}));
    assert_eq!(status["failures"], json!([]), "{status}");
    ok(&mut doc, json!({"op": "save", "path": hinge}));
    hinge
}

#[test]
fn a_smart_features_pattern_is_made_where_it_is_applied_as_many_as_it_comes_to() {
    for holes in [3, 4] {
        let dir = temp(&format!("pattern{holes}"));
        let (door, _) = parts(&dir);
        let hinge = patterned_hinge(&dir, holes);
        let mut doc = Document::default();
        ok(&mut doc, json!({"op": "new", "assembly": true}));
        ok(
            &mut doc,
            json!({"op": "insert", "path": door, "name": "Door-1", "fixed": true}),
        );
        ok(
            &mut doc,
            json!({"op": "insert", "path": hinge, "name": "Hinge-1", "at": [50, 60, 2]}),
        );
        let applied = ok(
            &mut doc,
            json!({"op": "apply_smart_features", "component": "Hinge-1", "references": [{"component": "Door-1", "face": {"normal": [0, 0, 1]}}]}),
        );
        assert_eq!(
            applied["applied"][0]["patterns"],
            json!([{"feature": "Smart-Feature1<Hinge-1> LPattern1", "count": holes}]),
            "{applied}"
        );
        let cut = 120_000.0 - door_volume(&doc);
        assert!(
            (cut - f64::from(holes) * HOLE).abs() < 1e-3,
            "{holes} holes: {cut} cut"
        );
    }
}

#[test]
fn a_smart_feature_is_patterned_only_with_smart_features() {
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [40, 20]}]}),
    );
    ok(
        &mut doc,
        json!({"op": "extrude", "sketch": "Sketch1", "depth": 10}),
    );
    ok(
        &mut doc,
        json!({"op": "sketch", "on": "top", "name": "Holes", "draw": [{"type": "circle", "center": [10, 10], "radius": 3}]}),
    );
    ok(
        &mut doc,
        json!({"op": "smart_feature", "name": "HingeHoles", "sketch": "Holes"}),
    );
    let reply = apply_json(
        &mut doc,
        &json!({"op": "linear_pattern", "features": ["Extrude1", "HingeHoles"], "direction": "x", "spacing": 50, "count": 2}),
        Undo::Step,
    );
    let text = reply.json.to_string();
    assert!(text.contains("only with other smart features"), "{text}");
}
