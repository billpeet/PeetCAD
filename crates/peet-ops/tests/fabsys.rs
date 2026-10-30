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
    // keeping its number as SolidWorks does, its mates and the equation kept, its
    // configuration the copy's of the same name.
    let r = ok(
        &mut doc,
        json!({"op": "replace", "component": "Door-2", "path": copy, "link": true}),
    );
    assert_eq!(r["component"]["name"], "J1 AL3 Door-2", "{r}");
    assert_eq!(r["component"]["configuration"], "LHS", "{r}");
    let mates = ok(&mut doc, json!({"op": "mates"}));
    assert_eq!(mates["mates"][0]["status"], "ok", "{mates}");
    assert_eq!(mates["mates"][0]["b"]["component"], "J1 AL3 Door-2");
    let dims = ok(&mut doc, json!({"op": "dimensions"}));
    assert_eq!(
        dims["equations"][0]["expression"], "\"Width@Sketch1@J1 AL3 Door-2\" - 20",
        "{dims}"
    );
    let all = ok(&mut doc, json!({"op": "components"}));
    assert_eq!(component(&all, "J1 AL3 Door-2")["min"][2], 32.0, "{all}");

    // A new instance is numbered one past the highest its file has had in the assembly,
    // as SolidWorks numbers it: numbers of deleted and replaced components aren't given
    // again (J1 AL3 Door-1 was deleted; Door-2 is now the copy).
    let r = ok(
        &mut doc,
        json!({"op": "insert", "path": copy, "at": [0, 0, 300]}),
    );
    assert_eq!(r["component"]["name"], "J1 AL3 Door-3", "{r}");
    let r = ok(
        &mut doc,
        json!({"op": "insert", "path": template, "at": [0, 0, 400]}),
    );
    assert_eq!(r["component"]["name"], "Door-3", "{r}");
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
fn a_ray_picks_the_face_seen_looking_along_it() {
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
    // As SolidWorks' SelectByRay: the face seen looking along the direction through the
    // point, so from below the part, looking down, it is still the top; a line that
    // misses the part meets nothing.
    let r = ok(
        &mut part,
        json!({"op": "name_face", "face": {"ray": {"from": [30, 20, -50], "direction": [0, 0, -1]}}, "name": "Seen"}),
    );
    assert_eq!(r["ok"], true);
    let faces = ok(&mut part, json!({"op": "faces"}));
    let seen = faces["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            f["names"]
                .as_array()
                .is_some_and(|n| n.contains(&json!("Seen")))
        })
        .unwrap();
    assert_eq!(seen["normal"], json!([0.0, 0.0, 1.0]), "{seen}");
    let e = error(
        &mut part,
        json!({"op": "measure", "a": {"face": {"ray": {"from": [100, 20, 10], "direction": [0, 0, 1]}}}}),
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
    ok(&mut doc, json!({"op": "delete", "mate": "Coincident1"}));
    // Starting on Low's underside and pointing down, it still finds High above: FabSys's
    // rays start on an enclosure's back, z = 0, point along -z and select its front, as
    // SolidWorks did in job 21441-1 (the Floor's front flange, the doors' mullions).
    let r = ok(
        &mut doc,
        json!({"op": "mate", "type": "coincident",
               "a": {"face": {"ray": {"from": [x, y, 0], "direction": [0, 0, -1]}}},
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
fn a_patterns_copies_follow_their_replaced_original() {
    let dir = temp("pattern");
    let template = block(&dir, "Gear tray bracket.peet");
    let copy = dir.join("J1 MS2 Gear tray bracket 1.peet");
    std::fs::copy(&template, &copy).unwrap();
    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "new", "assembly": true}));
    ok(&mut doc, json!({"op": "insert", "path": template}));
    ok(
        &mut doc,
        json!({"op": "component_pattern", "components": ["Gear tray bracket-1"], "type": "linear", "direction": "x", "spacing": 100, "count": 3}),
    );
    // A copy is its pattern's: it is replaced with its original.
    let e = error(
        &mut doc,
        json!({"op": "replace", "component": "Gear tray bracket-3", "path": copy, "link": true}),
    );
    assert!(e.contains("pattern"), "{e}");
    // FabSys's ReplaceComponents2 on the original: in SolidWorks the pattern's copies are
    // then instances of the new file too, keeping their numbers (Gear Tray Assembly's
    // Gear tray bracket-13 and -14 become the job's bracket-13 and -14).
    let r = ok(
        &mut doc,
        json!({"op": "replace", "component": "Gear tray bracket-1", "path": copy, "link": true}),
    );
    assert_eq!(
        r["component"]["name"], "J1 MS2 Gear tray bracket 1-1",
        "{r}"
    );
    let all = ok(&mut doc, json!({"op": "components"}));
    for (n, x) in [(1, 0.0), (2, 100.0), (3, 200.0)] {
        let c = component(&all, &format!("J1 MS2 Gear tray bracket 1-{n}"));
        assert_eq!(c["part"], "J1 MS2 Gear tray bracket 1", "{c}");
        assert_eq!(c["min"][0], x, "{c}");
    }
    assert_eq!(all["parts"].as_array().unwrap().len(), 1, "{all}");
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

/// A block 100 by 60 by 6 from the origin, with its faces named after the way they
/// look: `Up` (+Z), `Down`, `Left` (-X), `Right`, `Near` (-Y), `Far`.
fn named_block(dir: &std::path::Path, name: &str) -> String {
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
    for (face, normal) in [
        ("Up", [0, 0, 1]),
        ("Down", [0, 0, -1]),
        ("Left", [-1, 0, 0]),
        ("Right", [1, 0, 0]),
        ("Near", [0, -1, 0]),
        ("Far", [0, 1, 0]),
    ] {
        ok(
            &mut doc,
            json!({"op": "name_face", "face": {"normal": normal}, "name": face}),
        );
    }
    ok(&mut doc, json!({"op": "save", "path": path}));
    path
}

fn mate(a: (&str, &str), b: Value, align: &str) -> Value {
    json!({"op": "mate", "type": "coincident", "align": align,
           "a": {"component": a.0, "face": {"name": a.1}}, "b": b})
}

fn place(doc: &mut Document, name: &str) -> Value {
    let all = ok(doc, json!({"op": "components"}));
    let c = component(&all, name);
    json!([c["min"], c["max"]])
}

/// A component held by three plane mates to the assembly's planes stays where they put
/// it when another is mated to it the wrong way round: the newcomer is turned over, not
/// it (FabSys's job test2 turned its left wall over on inserting the stay bracket).
#[test]
fn a_component_its_mates_place_is_not_turned_over_by_a_later_mate() {
    let dir = temp("turned");
    let block = named_block(&dir, "Block.peet");
    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "new", "assembly": true}));
    ok(
        &mut doc,
        json!({"op": "insert", "path": block, "name": "Wall-1", "at": [40, 30, 20]}),
    );
    ok(
        &mut doc,
        json!({"op": "fix", "component": "Wall-1", "on": false}),
    );
    ok(
        &mut doc,
        mate(("Wall-1", "Down"), json!({"plane": "top"}), "anti_aligned"),
    );
    ok(
        &mut doc,
        mate(
            ("Wall-1", "Left"),
            json!({"plane": "right"}),
            "anti_aligned",
        ),
    );
    ok(
        &mut doc,
        mate(("Wall-1", "Near"), json!({"plane": "front"}), "aligned"),
    );
    let wall = place(&mut doc, "Wall-1");
    assert_eq!(wall, json!([[0.0, 0.0, 0.0], [100.0, 60.0, 6.0]]));

    // The bracket comes in looking up, as the wall does: put face to face with the
    // wall's top, it is the one turned over.
    ok(
        &mut doc,
        json!({"op": "insert", "path": block, "name": "Bracket-1", "at": [10, 10, 50]}),
    );
    ok(
        &mut doc,
        json!({"op": "fix", "component": "Bracket-1", "on": false}),
    );
    let r = ok(
        &mut doc,
        mate(
            ("Bracket-1", "Up"),
            json!({"component": "Wall-1", "face": {"name": "Up"}}),
            "anti_aligned",
        ),
    );
    assert_eq!(r["mate"]["status"], "ok", "{r}");
    assert_eq!(place(&mut doc, "Wall-1"), wall);
    let bracket = place(&mut doc, "Bracket-1");
    assert!(
        (bracket[0][2].as_f64().unwrap() - 6.0).abs() < 1e-6,
        "{bracket}"
    );
    // Its side to the wall's: the wall still doesn't move, and nothing does on hiding
    // one or rebuilding.
    let r = ok(
        &mut doc,
        mate(
            ("Bracket-1", "Left"),
            json!({"component": "Wall-1", "face": {"name": "Left"}}),
            "aligned",
        ),
    );
    assert_eq!(r["mate"]["status"], "ok", "{r}");
    let bracket = place(&mut doc, "Bracket-1");
    ok(
        &mut doc,
        json!({"op": "show", "component": "Bracket-1", "on": false}),
    );
    ok(&mut doc, json!({"op": "rebuild"}));
    assert_eq!(place(&mut doc, "Wall-1"), wall);
    assert_eq!(place(&mut doc, "Bracket-1"), bracket);
    let mates = ok(&mut doc, json!({"op": "mates"}));
    for m in mates["mates"].as_array().unwrap() {
        assert_eq!(m["status"], "ok", "{mates}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// A plane mate that asks for the one way round that the mates before it rule out fails,
/// rather than being had the other way round and said to hold.
#[test]
fn a_mate_that_cant_be_the_way_round_it_asks_fails() {
    let dir = temp("way-round");
    let block = named_block(&dir, "Block.peet");
    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "new", "assembly": true}));
    ok(
        &mut doc,
        json!({"op": "insert", "path": block, "name": "Wall-1"}),
    );
    ok(
        &mut doc,
        json!({"op": "fix", "component": "Wall-1", "on": false}),
    );
    ok(
        &mut doc,
        mate(("Wall-1", "Down"), json!({"plane": "top"}), "anti_aligned"),
    );
    ok(
        &mut doc,
        mate(
            ("Wall-1", "Left"),
            json!({"plane": "right"}),
            "anti_aligned",
        ),
    );
    // Turned to face the front plane the other way, it would be its own mirror image.
    let r = ok(
        &mut doc,
        mate(
            ("Wall-1", "Near"),
            json!({"plane": "front"}),
            "anti_aligned",
        ),
    );
    assert_eq!(r["mate"]["status"], "failed", "{r}");
    assert!(
        r["mate"]["message"]
            .as_str()
            .unwrap()
            .contains("anti-aligned"),
        "{r}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A door with two hinges whose leaves face opposite ways (Door Assembly's "R Hinge": its
/// third hinge's leaf, which no mate of the door's holds, was left turned round). Mated
/// face to face with a plane, the second leaf fails while the door is rigid, as in
/// SolidWorks; made flexible, the door's leaf is swung round its pin to suit the mate (as
/// SolidWorks does), not the door turned over, which would undo the first.
#[test]
fn a_flexible_sub_assemblys_free_leaf_swings_round_to_suit_a_mate() {
    let dir = temp("leaf");
    let path = |n: &str| dir.join(n).to_string_lossy().into_owned();
    let mut doc = Document::default();
    // The pin and the leaf, turning on it.
    ok(
        &mut doc,
        json!({"op": "sketch", "on": "top", "draw": [{"type": "circle", "center": [0, 0], "radius": 5}]}),
    );
    ok(
        &mut doc,
        json!({"op": "extrude", "sketch": "Sketch1", "depth": 25}),
    );
    ok(&mut doc, json!({"op": "save", "path": path("Pin.peet")}));
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [-10, -10], "to": [70, 10]}, {"type": "circle", "center": [0, 0], "radius": 5}]}),
    );
    ok(
        &mut doc,
        json!({"op": "extrude", "sketch": "Sketch1", "depth": 6}),
    );
    ok(&mut doc, json!({"op": "save", "path": path("Leaf.peet")}));
    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "new", "assembly": true}));
    ok(
        &mut doc,
        json!({"op": "insert", "path": path("Pin.peet"), "name": "Pin"}),
    );
    ok(
        &mut doc,
        json!({"op": "insert", "path": path("Leaf.peet"), "name": "Leaf"}),
    );
    ok(
        &mut doc,
        json!({"op": "mate", "type": "concentric", "a": {"component": "Pin", "face": {"at": [5, 0, 10]}}, "b": {"component": "Leaf", "face": {"at": [5, 0, 3]}}}),
    );
    ok(
        &mut doc,
        json!({"op": "mate", "type": "coincident", "flip": true, "a": {"component": "Pin", "face": {"normal": [0, 0, -1]}}, "b": {"component": "Leaf", "face": {"normal": [0, 0, -1]}}}),
    );
    ok(&mut doc, json!({"op": "save", "path": path("Hinge.peet")}));
    // The door: a slab and two hinges, the second's leaf turned round.
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [-20, -20], "to": [100, 220]}]}),
    );
    ok(
        &mut doc,
        json!({"op": "extrude", "sketch": "Sketch1", "depth": 5}),
    );
    ok(&mut doc, json!({"op": "save", "path": path("Slab.peet")}));
    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "new", "assembly": true}));
    ok(
        &mut doc,
        json!({"op": "insert", "path": path("Slab.peet"), "name": "Slab"}),
    );
    for (name, y) in [("H1", 0), ("H2", 200)] {
        ok(
            &mut doc,
            json!({"op": "insert", "path": path("Hinge.peet"), "name": name, "at": [0, y, 5], "fixed": true}),
        );
        ok(&mut doc, json!({"op": "flexible", "component": name}));
    }
    ok(
        &mut doc,
        json!({"op": "place", "component": ["H2", "Leaf"], "rotate": {"axis": "z", "angle": 180}}),
    );
    ok(&mut doc, json!({"op": "save", "path": path("Door.peet")}));

    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "new", "assembly": true}));
    ok(
        &mut doc,
        json!({"op": "insert", "path": path("Door.peet"), "name": "Door", "at": [300, 50, 0]}),
    );
    ok(
        &mut doc,
        json!({"op": "fix", "component": "Door", "on": false}),
    );
    // Each leaf's end, face to face with the right plane.
    let leaf_end = |hinge: &str| {
        json!({"op": "mate", "type": "coincident", "align": "anti_aligned",
               "a": {"component": ["Door", hinge, "Leaf"], "face": {"at": [70, 0, 3]}},
               "b": {"plane": "right"}})
    };
    let r = ok(&mut doc, leaf_end("H1"));
    assert_eq!(r["mate"]["status"], "ok", "{r}");
    let r = ok(&mut doc, leaf_end("H2"));
    assert_eq!(r["mate"]["status"], "failed", "{r}");
    ok(&mut doc, json!({"op": "flexible", "component": "Door"}));
    let mates = ok(&mut doc, json!({"op": "mates"}));
    for m in mates["mates"].as_array().unwrap() {
        assert_eq!(m["status"], "ok", "{mates}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// FabSys's `SetDimension("Notch@Sketch42@…", -203)`: SolidWorks takes a negative value
/// for a distance by reversing it, the geometry going to the other side.
#[test]
fn a_negative_distance_reverses_the_dimension() {
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op": "sketch", "on": "top", "draw": [
            {"type": "line", "from": [0, 0], "to": [0, 40], "construction": true, "as": "centre"},
            {"type": "fix", "of": ["centre"]},
            {"type": "circle", "center": [-30, 20], "radius": 4, "as": "hole"},
            {"type": "distance", "of": ["hole.center", "centre"], "value": 30, "name": "Notch"},
            {"type": "vertical_distance", "of": ["centre.start", "hole.center"], "value": 20},
            {"type": "radius", "of": ["hole"], "value": 4, "name": "R"},
        ]}),
    );
    let centre = |doc: &mut Document| {
        let s = ok(doc, json!({"op": "feature", "feature": "Sketch1"}));
        let hole = s["entities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["type"] == "circle")
            .unwrap()
            .clone();
        hole["at"][0].as_f64().unwrap()
    };
    ok(
        &mut doc,
        json!({"op": "set", "name": "Notch@Sketch1", "value": -25}),
    );
    assert!((centre(&mut doc) - 25.0).abs() < 1e-9);
    let dims = ok(&mut doc, json!({"op": "dimensions", "feature": "Sketch1"}));
    assert!(dims.to_string().contains("25"), "{dims}");
    // It stays on that side: a positive value is a size, as before.
    ok(
        &mut doc,
        json!({"op": "set", "name": "Notch@Sketch1", "value": 12}),
    );
    assert!((centre(&mut doc) - 12.0).abs() < 1e-9);
    // Negative again: back.
    ok(
        &mut doc,
        json!({"op": "set_dimension", "sketch": "Sketch1", "name": "Notch", "value": -12}),
    );
    assert!((centre(&mut doc) + 12.0).abs() < 1e-9);
    // A radius has no other side.
    let e = error(
        &mut doc,
        json!({"op": "set", "name": "R@Sketch1", "value": -4}),
    );
    assert!(e.contains("greater than zero"), "{e}");
}

/// SolidWorks finds features, reference planes and named faces without regard to case
/// (FabSys's script asks for "wiv Mid Plane" of a part with "WIV Mid Plane").
#[test]
fn names_are_found_in_any_case_where_only_one_is_alike() {
    let dir = temp("case");
    let block = named_block(&dir, "Block.peet");
    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "open", "path": block}));
    ok(
        &mut doc,
        json!({"op": "plane", "from": "top", "distance": 3, "name": "WIV Mid Plane"}),
    );
    ok(&mut doc, json!({"op": "save"}));
    ok(
        &mut doc,
        json!({"op": "suppress", "feature": "extrude1", "on": false}),
    );
    let mut asm = Document::default();
    ok(&mut asm, json!({"op": "new", "assembly": true}));
    ok(
        &mut asm,
        json!({"op": "insert", "path": block, "name": "Wiv-1", "fixed": false}),
    );
    let r = ok(
        &mut asm,
        json!({"op": "mate", "type": "coincident", "align": "aligned",
               "a": {"component": "wiv-1", "plane": "wiv mid plane"}, "b": {"plane": "top"}}),
    );
    assert_eq!(r["mate"]["status"], "ok", "{r}");
    let r = ok(
        &mut asm,
        mate(("Wiv-1", "LEFT"), json!({"plane": "right"}), "anti_aligned"),
    );
    assert_eq!(r["mate"]["status"], "ok", "{r}");
    ok(&mut asm, json!({"op": "suppress", "mate": "coincident1"}));
    // Two alike but for case: neither is meant.
    ok(
        &mut doc,
        json!({"op": "plane", "from": "top", "distance": 4, "name": "wiv mid PLANE"}),
    );
    let e = error(
        &mut doc,
        json!({"op": "suppress", "feature": "WIV MID PLANE"}),
    );
    assert!(e.contains("WIV MID PLANE"), "{e}");
    let _ = std::fs::remove_dir_all(dir);
}

/// A bend deduction too big for a shallow bend (FabSys sets 3.325 mm on a part whose 45°
/// flange then works out at a negative allowance: MS3 Forklift Tine lifting plate) folds the
/// part all the same, as SolidWorks does, with a warning, and its flat pattern is refused,
/// as SolidWorks' flat pattern fails.
#[test]
fn a_bend_too_shallow_for_its_deduction_folds_but_has_no_flat_pattern() {
    let dir = temp("unfoldable");
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [100, 60]}]}),
    );
    ok(
        &mut doc,
        json!({"op": "base_flange", "sketch": "Sketch1", "thickness": 2, "radius": 0.1, "bend": {"deduction": 3.325}}),
    );
    ok(
        &mut doc,
        json!({"op": "edge_flange", "name": "Lip", "length": 20, "angle": 45, "edges": [{"between": [[0, 0, 2], [100, 0, 2]]}]}),
    );
    let features = ok(&mut doc, json!({"op": "features"}));
    let lip = features["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "Lip")
        .unwrap()
        .clone();
    assert_eq!(lip["status"], "warning", "{lip}");
    assert!(
        lip["message"].as_str().unwrap().contains("no flat pattern"),
        "{lip}"
    );
    let bodies = ok(&mut doc, json!({"op": "bodies"}));
    assert!(bodies["bodies"][0]["volume_mm3"].as_f64().unwrap() > 100.0 * 60.0 * 2.0);
    let dxf = dir.join("flat.dxf").to_string_lossy().into_owned();
    let e = error(&mut doc, json!({"op": "export", "path": dxf}));
    assert!(e.contains("no flat pattern"), "{e}");
    let _ = std::fs::remove_dir_all(dir);
}

/// A reference plane made in an assembly on a component's face follows that component as
/// the mates move it (SolidWorks' Plinth PFC Assembly's FrontPlane, on its back channel,
/// which FabSys mates the plinth by): `in_context` with no `component` makes the
/// assembly's own plane follow.
#[test]
fn an_assembly_plane_on_a_components_face_follows_it() {
    let dir = temp("own-plane");
    let block = named_block(&dir, "Block.peet");
    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "new", "assembly": true}));
    ok(
        &mut doc,
        json!({"op": "insert", "path": block, "name": "Front-1", "fixed": true}),
    );
    ok(
        &mut doc,
        json!({"op": "insert", "path": block, "name": "Back-1", "at": [0, 200, 0]}),
    );
    ok(
        &mut doc,
        json!({"op": "fix", "component": "Back-1", "on": false}),
    );
    ok(
        &mut doc,
        json!({"op": "mate", "type": "coincident", "align": "aligned",
               "a": {"component": "Back-1", "face": {"name": "Down"}}, "b": {"component": "Front-1", "face": {"name": "Down"}}}),
    );
    ok(
        &mut doc,
        json!({"op": "mate", "type": "coincident", "align": "aligned",
               "a": {"component": "Back-1", "face": {"name": "Left"}}, "b": {"component": "Front-1", "face": {"name": "Left"}}}),
    );
    ok(
        &mut doc,
        json!({"op": "mate", "type": "distance", "distance": 100, "name": "Gap",
               "a": {"component": "Back-1", "face": {"name": "Near"}}, "b": {"component": "Front-1", "face": {"name": "Far"}}}),
    );
    // The plane, made on a standard plane, then made to be on the back block's far face
    // (it is that face offset by its distance, here none)
    ok(
        &mut doc,
        json!({"op": "plane", "from": "front", "distance": 0, "name": "BackPlane"}),
    );
    let made = ok(
        &mut doc,
        json!({"op": "in_context", "feature": "BackPlane",
               "from": {"component": "Back-1", "face": {"name": "Far"}}}),
    );
    assert_eq!(made["plane_of"], "BackPlane", "{made}");
    // Another block, mated to the plane
    ok(
        &mut doc,
        json!({"op": "insert", "path": block, "name": "Stop-1", "at": [300, 0, 0]}),
    );
    ok(
        &mut doc,
        json!({"op": "fix", "component": "Stop-1", "on": false}),
    );
    ok(
        &mut doc,
        // (The plane faces the way it was made, as the front plane does: -Y)
        json!({"op": "mate", "type": "coincident", "align": "aligned",
               "a": {"component": "Stop-1", "face": {"name": "Near"}}, "b": {"plane": "BackPlane"}}),
    );
    let near = |doc: &mut Document| {
        let all = ok(doc, json!({"op": "components"}));
        component(&all, "Stop-1")["min"][1].as_f64().unwrap()
    };
    assert!((near(&mut doc) - 220.0).abs() < 1e-6, "{}", near(&mut doc));
    // The back block moves: the plane, and what is mated to it, go with it
    ok(
        &mut doc,
        json!({"op": "edit_mate", "mate": "Gap", "distance": 300}),
    );
    assert!((near(&mut doc) - 420.0).abs() < 1e-6, "{}", near(&mut doc));
    let e = error(
        &mut doc,
        json!({"op": "in_context", "sketch": "Sketch1", "from": {"component": "Back-1", "face": {"name": "Far"}}}),
    );
    assert!(e.contains("component"), "{e}");
    let _ = std::fs::remove_dir_all(dir);
}

/// A reference plane suppressed in its part is still a mate end where it would be, as
/// SolidWorks mates to it (FabSys mates Esc Surround Mid's MidPlaneV to Esc Surround R
/// Bottom's suppressed Plane1); a sketch on it is suppressed with it.
#[test]
fn a_suppressed_plane_is_mated_to_where_it_would_be() {
    let dir = temp("suppressed-plane");
    let path = dir.join("Bottom.peet").to_string_lossy().into_owned();
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [100, 60]}]}),
    );
    ok(
        &mut doc,
        json!({"op": "extrude", "sketch": "Sketch1", "depth": 6}),
    );
    ok(
        &mut doc,
        json!({"op": "plane", "from": "right", "distance": 50, "name": "Plane1"}),
    );
    ok(
        &mut doc,
        json!({"op": "sketch", "on": "Plane1", "name": "On it", "draw": [{"type": "circle", "center": [0, 0], "radius": 3}]}),
    );
    ok(
        &mut doc,
        json!({"op": "suppress", "feature": "Plane1", "on": true}),
    );
    let features = ok(&mut doc, json!({"op": "features"}));
    let status = |name: &str| {
        features["features"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == name)
            .unwrap()["status"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(status("Plane1"), "suppressed");
    assert_eq!(status("On it"), "suppressed");
    ok(&mut doc, json!({"op": "save", "path": path}));
    let block = block(&dir, "Block.peet");
    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "new", "assembly": true}));
    ok(
        &mut doc,
        json!({"op": "insert", "path": path, "name": "Bottom-1", "fixed": true, "at": [200, 0, 0]}),
    );
    ok(
        &mut doc,
        json!({"op": "insert", "path": block, "name": "Mid-1", "at": [0, 100, 0]}),
    );
    let reply = ok(
        &mut doc,
        json!({"op": "mate", "type": "coincident", "a": {"component": "Mid-1", "plane": "right"},
               "b": {"component": "Bottom-1", "plane": "Plane1"}}),
    );
    assert_eq!(reply["mate"]["status"], "ok", "{reply}");
    let at = ok(&mut doc, json!({"op": "components"}));
    let x = component(&at, "Mid-1")["at"][0].as_f64().unwrap();
    assert!((x - 250.0).abs() < 1e-6, "{at}");
    let _ = std::fs::remove_dir_all(dir);
}
