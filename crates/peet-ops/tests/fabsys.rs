//! What FabSys needs of PeetCAD to drive it as it drives SolidWorks: components named
//! after the files they come from, components pointed at copies of their templates,
//! planes made in an assembly and SolidWorks' own standard planes, faces picked by a ray,
//! a part's sheet metal set from its assembly, and a forced rebuild.

use std::path::PathBuf;

use peet_document::Document;
use peet_ops::{Undo, apply_json};
use serde_json::{Value, json};

fn ok(doc: &mut Document, op: Value) -> Value {
    let reply = apply_json(doc, &op, Undo::Step);
    assert!(reply.ok, "{op} failed: {}", reply.json["error"]);
    reply.json
}

fn error(doc: &mut Document, op: Value) -> String {
    let reply = apply_json(doc, &op, Undo::Step);
    assert!(!reply.ok, "{op} should have failed: {}", reply.json);
    reply.json["error"].as_str().unwrap().to_owned()
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("peet-fabsys-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A sheet metal plate 100 by 60 with a hole, in two configurations, saved as `name`.
fn plate(dir: &std::path::Path, name: &str) -> String {
    let path = dir.join(name).to_string_lossy().into_owned();
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op": "sketch", "on": "top", "draw": [
            {"type": "rectangle", "from": [0, 0], "to": [100, 60], "as": "r"},
            {"type": "coincident", "of": ["r.bottom.start", "origin"]},
            {"type": "length", "of": ["r.bottom"], "value": 100, "name": "Width"},
            {"type": "length", "of": ["r.left"], "value": 60},
            {"type": "circle", "center": [50, 30], "radius": 5},
        ]}),
    );
    ok(
        &mut doc,
        json!({"op": "base_flange", "sketch": "Sketch1", "thickness": 2, "radius": 1, "bend": {"deduction": 3.3}}),
    );
    ok(&mut doc, json!({"op": "add_configuration", "name": "LHS"}));
    ok(
        &mut doc,
        json!({"op": "configuration", "configuration": "Default"}),
    );
    ok(&mut doc, json!({"op": "save", "path": path}));
    path
}

/// A block 100 by 60 by 6 from the origin, saved as `name`.
fn block(dir: &std::path::Path, name: &str) -> String {
    let path = dir.join(name).to_string_lossy().into_owned();
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [100, 60]}]}),
    );
    ok(
        &mut doc,
        json!({"op": "extrude", "sketch": "Sketch1", "depth": 6}),
    );
    ok(&mut doc, json!({"op": "save", "path": path}));
    path
}

fn component<'a>(components: &'a Value, name: &str) -> &'a Value {
    components["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no {name} in {components}"))
}

#[test]
fn components_are_named_after_their_files_and_follow_replaced_files() {
    let dir = temp("names");
    // The template, and FabSys's copy of it for a job: the same part in another file.
    let template = plate(&dir, "Door.peet");
    let copy = dir.join("J1 AL3 Door.peet");
    std::fs::copy(&template, &copy).unwrap();

    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "new", "assembly": true}));
    // Inserted from a file, a component is named after the file, as SolidWorks names it.
    let r = ok(&mut doc, json!({"op": "insert", "path": template}));
    assert_eq!(r["component"]["name"], "Door-1");
    let r = ok(
        &mut doc,
        json!({"op": "insert", "path": template, "at": [0, 0, 50], "configuration": "LHS"}),
    );
    assert_eq!(r["component"]["name"], "Door-2");
    let r = ok(
        &mut doc,
        json!({"op": "insert", "path": copy, "at": [0, 0, 100]}),
    );
    assert_eq!(r["component"]["name"], "J1 AL3 Door-1");
    assert_eq!(r["component"]["part"], "J1 AL3 Door");
    ok(
        &mut doc,
        json!({"op": "delete", "component": "J1 AL3 Door-1"}),
    );

    // A mate on Door-2, and an equation of the assembly reading Door-2.
    ok(
        &mut doc,
        json!({"op": "mate", "type": "distance", "distance": 30,
               "a": {"component": "Door-1", "face": {"normal": [0, 0, 1]}},
               "b": {"component": "Door-2", "face": {"normal": [0, 0, -1]}}}),
    );
    ok(
        &mut doc,
        json!({"op": "set", "name": "Width@Sketch1@Door-1", "value": "\"Width@Sketch1@Door-2\" - 20"}),
    );

    // Pointed at the job's copy (FabSys's ReplaceComponents2): renamed after the copy,
    // its mates and the equation kept, its configuration the copy's of the same name.
    let r = ok(
        &mut doc,
        json!({"op": "replace", "component": "Door-2", "path": copy, "link": true}),
    );
    assert_eq!(r["component"]["name"], "J1 AL3 Door-1", "{r}");
    assert_eq!(r["component"]["configuration"], "LHS", "{r}");
    let mates = ok(&mut doc, json!({"op": "mates"}));
    assert_eq!(mates["mates"][0]["status"], "ok", "{mates}");
    assert_eq!(mates["mates"][0]["b"]["component"], "J1 AL3 Door-1");
    let dims = ok(&mut doc, json!({"op": "dimensions"}));
    assert_eq!(
        dims["equations"][0]["expression"], "\"Width@Sketch1@J1 AL3 Door-1\" - 20",
        "{dims}"
    );
    let all = ok(&mut doc, json!({"op": "components"}));
    assert_eq!(component(&all, "J1 AL3 Door-1")["min"][2], 32.0, "{all}");
    let parts = all["parts"].as_array().unwrap();
    let linked = parts.iter().find(|p| p["name"] == "J1 AL3 Door").unwrap();
    assert_eq!(linked["link_status"], "current", "{all}");

    // A component someone named stays named.
    ok(
        &mut doc,
        json!({"op": "rename", "component": "Door-1", "name": "Left door"}),
    );
    let r = ok(
        &mut doc,
        json!({"op": "replace", "component": "Left door", "path": copy, "link": true}),
    );
    assert_eq!(r["component"]["name"], "Left door");

    // link needs a file, and absolute needs link.
    let e = error(
        &mut doc,
        json!({"op": "replace", "component": "Left door", "sample": "bracket", "link": true}),
    );
    assert!(e.contains("path"), "{e}");
    let e = error(
        &mut doc,
        json!({"op": "replace", "component": "Left door", "path": copy, "absolute": true}),
    );
    assert!(e.contains("link"), "{e}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_assembly_has_planes_of_its_own_and_solidworks_standard_planes() {
    let dir = temp("planes");
    let block = block(&dir, "Block.peet");
    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "new", "assembly": true}));
    ok(
        &mut doc,
        json!({"op": "insert", "path": block, "name": "Fixed"}),
    );
    ok(
        &mut doc,
        json!({"op": "insert", "path": block, "name": "Loose", "at": [300, 300, 300]}),
    );

    // SolidWorks' standard planes, as SolidWorks has them in its coordinates: Top Plane
    // looks up +Y (PeetCAD's front looks along -Y).
    let r = ok(&mut doc, json!({"op": "solidworks_planes"}));
    assert_eq!(r["created"].as_array().unwrap().len(), 3, "{r}");
    let again = ok(&mut doc, json!({"op": "solidworks_planes"}));
    assert_eq!(again["added"], json!([]));

    // InsertPlane("EnclBasePlane", "Top Plane", -400): 400 below, along -Y.
    ok(
        &mut doc,
        json!({"op": "plane", "from": "Top Plane", "distance": -400, "name": "EnclBasePlane"}),
    );
    let features = ok(&mut doc, json!({"op": "features"}));
    assert_eq!(
        features["features"][3]["name"], "EnclBasePlane",
        "{features}"
    );

    // A component's face onto it, against it: the face looking down -Y goes to Y = -400,
    // the component above it.
    let r = ok(
        &mut doc,
        json!({"op": "mate", "type": "coincident",
               "a": {"component": "Loose", "face": {"normal": [0, -1, 0]}},
               "b": {"plane": "EnclBasePlane"}}),
    );
    assert_eq!(r["mate"]["status"], "ok", "{r}");
    assert_eq!(r["mate"]["b"]["plane"], "EnclBasePlane", "{r}");
    let c = &r["moved"][0];
    assert!((c["min"][1].as_f64().unwrap() + 400.0).abs() < 1e-6, "{c}");

    // With a distance from SolidWorks' Front Plane (XY, looking +Z): 25 in front of it.
    let r = ok(
        &mut doc,
        json!({"op": "mate", "type": "distance", "distance": 25,
               "a": {"plane": "Front Plane"},
               "b": {"component": "Loose", "face": {"normal": [0, 0, -1]}}}),
    );
    assert_eq!(r["mate"]["status"], "ok", "{r}");
    assert!(
        (r["moved"][0]["min"][2].as_f64().unwrap() - 25.0).abs() < 1e-6,
        "{r}"
    );

    // Saved and opened again, the mates are on the same planes.
    let bytes = doc.save_bytes(false).unwrap();
    let opened = peet_io::document::open(&bytes).unwrap();
    let mut again = Document::from_model(opened.model, None);
    let mates = ok(&mut again, json!({"op": "mates"}));
    assert_eq!(mates["mates"][0]["status"], "ok", "{mates}");

    // An end with no component is on the assembly's planes only.
    let e = error(
        &mut doc,
        json!({"op": "mate", "type": "coincident", "a": {"plane": "Nowhere"},
               "b": {"component": "Loose", "face": {"normal": [0, 0, 1]}}}),
    );
    assert!(e.contains("Nowhere"), "{e}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_plane_named_as_a_standard_plane_is_the_standard_one_where_there_is_no_such_plane() {
    let mut doc = Document::default();
    // No reference plane called "Front Plane": PeetCAD's front (XZ).
    let r = ok(
        &mut doc,
        json!({"op": "sketch", "on": "Front Plane", "draw": [{"type": "circle", "center": [0, 0], "radius": 5}]}),
    );
    assert_eq!(r["plane"]["normal"], json!([0.0, -1.0, 0.0]));
    // With SolidWorks' planes: SolidWorks' Front Plane (XY).
    ok(&mut doc, json!({"op": "solidworks_planes"}));
    let r = ok(
        &mut doc,
        json!({"op": "sketch", "on": "Front Plane", "draw": [{"type": "circle", "center": [0, 0], "radius": 5}]}),
    );
    assert_eq!(r["plane"]["normal"], json!([0.0, 0.0, 1.0]), "{r}");
    let r = ok(
        &mut doc,
        json!({"op": "sketch", "on": "Top Plane", "draw": [{"type": "circle", "center": [0, 0], "radius": 5}]}),
    );
    let n = &r["plane"]["normal"];
    assert!((n[1].as_f64().unwrap() - 1.0).abs() < 1e-9, "{r}");
    // The bare words are always PeetCAD's own.
    let r = ok(
        &mut doc,
        json!({"op": "sketch", "on": "top", "draw": [{"type": "circle", "center": [0, 0], "radius": 5}]}),
    );
    assert_eq!(r["plane"]["normal"], json!([0.0, 0.0, 1.0]));
}

#[test]
fn a_ray_picks_the_first_face_it_meets() {
    // In a part: a ray down onto a plate 6 thick meets its top.
    let mut part = Document::default();
    ok(
        &mut part,
        json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [60, 40]}]}),
    );
    ok(
        &mut part,
        json!({"op": "extrude", "sketch": "Sketch1", "depth": 6}),
    );
    let r = ok(
        &mut part,
        json!({"op": "name_face", "face": {"ray": {"from": [30, 20, 100], "direction": [0, 0, -1]}}, "name": "TopFace"}),
    );
    assert_eq!(r["ok"], true);
    let faces = ok(&mut part, json!({"op": "faces"}));
    let top = faces["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            f["names"]
                .as_array()
                .is_some_and(|n| n.contains(&json!("TopFace")))
        })
        .unwrap();
    assert_eq!(top["normal"], json!([0.0, 0.0, 1.0]), "{top}");
    // Starting on a face, it meets that face; pointing away from the part, none.
    let r = ok(
        &mut part,
        json!({"op": "measure", "a": {"face": {"ray": {"from": [30, 20, 0], "direction": [0, 0, -1]}}}}),
    );
    assert_eq!(r["area_mm2"], 2400.0);
    let e = error(
        &mut part,
        json!({"op": "measure", "a": {"face": {"ray": {"from": [30, 20, 10], "direction": [0, 0, 1]}}}}),
    );
    assert!(e.contains("meets no face"), "{e}");

    // In an assembly, with no component: the first face of any component shown, in the
    // assembly's coordinates (FabSys's AddFace with a point and a direction).
    let dir = temp("ray");
    let block = block(&dir, "Block.peet");
    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "new", "assembly": true}));
    ok(
        &mut doc,
        json!({"op": "insert", "path": block, "name": "Low"}),
    );
    ok(
        &mut doc,
        json!({"op": "insert", "path": block, "name": "High", "at": [0, 0, 200], "fixed": true}),
    );
    ok(
        &mut doc,
        json!({"op": "insert", "path": block, "name": "Moving", "at": [500, 0, 0]}),
    );
    let (x, y) = (50.0, 30.0);
    let r = ok(
        &mut doc,
        json!({"op": "mate", "type": "coincident",
               "a": {"face": {"ray": {"from": [x, y, 1000], "direction": [0, 0, -1]}}},
               "b": {"component": "Moving", "face": {"normal": [0, 0, -1]}}}),
    );
    assert_eq!(r["mate"]["a"]["component"], "High", "{r}");
    // Hidden, it is passed through.
    ok(
        &mut doc,
        json!({"op": "show", "component": "High", "on": false}),
    );
    ok(&mut doc, json!({"op": "delete", "mate": "Coincident1"}));
    let r = ok(
        &mut doc,
        json!({"op": "mate", "type": "coincident",
               "a": {"face": {"ray": {"from": [x, y, 1000], "direction": [0, 0, -1]}}},
               "b": {"component": "Moving", "face": {"normal": [0, 0, -1]}}}),
    );
    assert_eq!(r["mate"]["a"]["component"], "Low", "{r}");
    // The block's top face, 6 up: the moving one's bottom is on it.
    let all = ok(&mut doc, json!({"op": "components"}));
    assert_eq!(component(&all, "Moving")["min"][2], 6.0, "{all}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn sheet_metal_is_set_on_a_part_and_on_a_components_part() {
    let dir = temp("sheet");
    let path = plate(&dir, "Panel.peet");
    let mut part = Document::default();
    ok(&mut part, json!({"op": "open", "path": path}));
    let r = ok(
        &mut part,
        json!({"op": "sheet_metal", "thickness": 3, "bend": {"deduction": 4.5}}),
    );
    assert_eq!(r["feature"]["name"], "Base-Flange1", "{r}");
    assert_eq!(
        r["sheet_metal"],
        json!({"feature": "Base-Flange1", "thickness": 3.0, "radius": 1.0, "bend": {"deduction": 4.5}}),
        "{r}"
    );
    let bodies = ok(&mut part, json!({"op": "bodies"}));
    assert_eq!(bodies["bodies"][0]["size"][2], 3.0);
    let e = error(&mut part, json!({"op": "sheet_metal"}));
    assert!(e.contains("thickness"), "{e}");
    let e = error(
        &mut part,
        json!({"op": "sheet_metal", "component": "X", "thickness": 1}),
    );
    assert!(e.contains("is a part"), "{e}");

    // From an assembly, a linked component's part: every instance, and its file.
    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "new", "assembly": true}));
    ok(
        &mut doc,
        json!({"op": "insert", "path": path, "link": true}),
    );
    ok(
        &mut doc,
        json!({"op": "insert", "component": "Panel-1", "at": [0, 0, 50]}),
    );
    let r = ok(
        &mut doc,
        json!({"op": "sheet_metal", "component": "Panel-2", "thickness": 1.5, "radius": 0.1, "bend": {"deduction": 2.5}}),
    );
    assert_eq!(r["sheet_metal"]["thickness"], 1.5, "{r}");
    assert_eq!(r["component"]["name"], "Panel-2", "{r}");
    assert!(
        r["written"].as_str().unwrap().ends_with("Panel.peet"),
        "{r}"
    );
    let all = ok(&mut doc, json!({"op": "components"}));
    assert_eq!(component(&all, "Panel-1")["max"][2], 1.5, "{all}");
    let mut file = Document::default();
    ok(&mut file, json!({"op": "open", "path": path}));
    let dims = ok(
        &mut file,
        json!({"op": "dimensions", "feature": "Base-Flange1"}),
    );
    let value = |field: &str| {
        dims["dimensions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["field"] == field)
            .map(|d| d["value"].clone())
    };
    assert_eq!(value("thickness"), Some(json!(1.5)), "{dims}");
    assert_eq!(value("deduction"), Some(json!(2.5)), "{dims}");
    let e = error(&mut doc, json!({"op": "sheet_metal", "thickness": 1}));
    assert!(e.contains("component"), "{e}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_forced_rebuild_rebuilds_and_says_how_the_document_is() {
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [60, 40]}]}),
    );
    ok(
        &mut doc,
        json!({"op": "extrude", "sketch": "Sketch1", "depth": 6}),
    );
    let before = doc.model.clone();
    let r = ok(&mut doc, json!({"op": "rebuild"}));
    assert_eq!(
        (r["op"].as_str(), r["features"].as_u64()),
        (Some("rebuild"), Some(2))
    );
    assert_eq!(r["failures"], json!([]));
    assert_eq!(doc.model, before, "a rebuild changes nothing");
}
