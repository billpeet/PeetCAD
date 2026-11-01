//! Linear patterns that run up to a reference (SolidWorks' "up to reference").
use peet_document::Document;
use peet_kernel::validate::measure;
use peet_ops::{Undo, apply_json};
use serde_json::{Value, json};

fn ok(doc: &mut Document, op: Value) -> Value {
    let reply = apply_json(doc, &op, Undo::Step);
    assert!(reply.ok, "{op}: {}", reply.json);
    reply.json
}

/// A 100 x 100 x 4 plate on the top plane with a 2 mm hole at x = 15, z = 0, and a
/// reference plane `End` square to x at x = 40.
fn plate() -> Document {
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op":"sketch", "on":"top", "draw":[{"type":"rectangle", "from":[-50,-50], "to":[50,50]}]}),
    );
    ok(
        &mut doc,
        json!({"op":"extrude", "sketch":"Sketch1", "depth":4}),
    );
    ok(
        &mut doc,
        json!({"op":"sketch", "name":"Seed", "on":"top", "draw":[{"type":"circle", "center":[15,0], "radius":1}]}),
    );
    ok(
        &mut doc,
        json!({"op":"cut", "sketch":"Seed", "end":"mid_plane", "depth":20}),
    );
    ok(
        &mut doc,
        json!({"op":"plane", "from":"right", "distance":40, "name":"End"}),
    );
    doc
}

fn holes(doc: &Document) -> f64 {
    let actual: f64 = doc.bodies.iter().map(|b| measure::volume(&b.solid)).sum();
    (40000.0 - actual) / (4.0 * std::f64::consts::PI)
}

/// The value of a pattern's dimension, by its field, as `dimensions` reports it.
fn field_value(doc: &mut Document, feature: &str, field: &str) -> f64 {
    let dims = ok(doc, json!({"op":"dimensions", "feature":feature}));
    dims["dimensions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["field"] == field)
        .and_then(|d| d["value"].as_f64())
        .unwrap_or_else(|| panic!("no {field} in {dims}"))
}

fn assert_holes(doc: &Document, n: u32) {
    let got = holes(doc);
    assert!((got - f64::from(n)).abs() < 1e-6, "{got} holes, not {n}");
}

#[test]
fn the_count_follows_the_reference() {
    let mut doc = plate();
    assert_holes(&doc, 1);
    // From x = 15 to the plane at 40, less 2: 23 mm, room for copies at 20, 25, 30 and 35
    ok(
        &mut doc,
        json!({"op":"linear_pattern", "features":["Cut-Extrude1"], "direction":"x", "spacing":5,
               "up_to":{"plane":"End"}, "up_to_offset":2}),
    );
    assert_holes(&doc, 5);
    // The count it reports is the one worked out, as SolidWorks shows it
    assert_eq!(field_value(&mut doc, "LPattern1", "count"), 5.0);
    // A copy exactly at the end counts: 25 mm is five spacings
    ok(
        &mut doc,
        json!({"op":"edit", "feature":"LPattern1", "up_to_offset":0}),
    );
    assert_holes(&doc, 6);
    // Moving the reference moves the end of the row
    ok(
        &mut doc,
        json!({"op":"edit", "feature":"End", "distance":31}),
    );
    assert_holes(&doc, 4);
    // Past the reference by the offset
    ok(
        &mut doc,
        json!({"op":"edit", "feature":"LPattern1", "up_to_offset":5, "up_to_reverse":true}),
    );
    assert_holes(&doc, 5);
    let query = ok(&mut doc, json!({"op":"feature", "feature":"LPattern1"}));
    assert_eq!(query["fields"]["up_to"]["plane"], "End");
    assert_eq!(query["fields"]["up_to_keeps"], "spacing");
    // Back to the count as given
    ok(
        &mut doc,
        json!({"op":"edit", "feature":"LPattern1", "up_to":null, "count":2}),
    );
    assert_holes(&doc, 2);
}

#[test]
fn the_spacing_follows_the_reference_when_the_count_is_given() {
    let mut doc = plate();
    // Three holes from x = 15 to a vertex of the plate at x = 50, less 5: spaced 15 apart
    ok(
        &mut doc,
        json!({"op":"linear_pattern", "features":["Cut-Extrude1"], "direction":"x", "count":3,
               "up_to":{"point":{"at":[50,50,0]}}, "up_to_offset":5, "up_to_keeps":"count"}),
    );
    assert_holes(&doc, 3);
    // The spacing it reports, and that expressions read, is the one worked out (SolidWorks'
    // Back Wall: D3@LPattern1 is 280.65 over the wall, not the 270.98 last stored)
    assert!((field_value(&mut doc, "LPattern1", "spacing") - 15.0).abs() < 1e-9);
    ok(
        &mut doc,
        json!({"op":"set_parameter", "name":"read", "value":"\"spacing@LPattern1\" * 2"}),
    );
    let params = ok(&mut doc, json!({"op":"parameters"}));
    let read = params["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "read")
        .unwrap()["value"]
        .clone();
    assert!(read.to_string().contains("30"), "{read}");
    let faces = ok(&mut doc, json!({"op":"faces"}));
    let round: Vec<f64> = faces["faces"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["surface"] == "cylinder")
        .filter_map(|f| f["center"][0].as_f64())
        .collect();
    for x in [15.0, 30.0, 45.0] {
        assert!(
            round.iter().any(|r| (r - x).abs() < 1e-6),
            "a hole at x = {x}: {round:?}"
        );
    }
}

#[test]
fn a_count_an_expression_gives_is_rounded_as_solidworks_rounds_it() {
    // SolidWorks' Back Wall: "D1@LPattern1" = ("Base Height" - 50) / 250, 7.8 at 2000 mm, is 8
    let mut doc = plate();
    ok(
        &mut doc,
        json!({"op":"set_parameter", "name":"holes", "value":"2.6"}),
    );
    ok(
        &mut doc,
        json!({"op":"linear_pattern", "features":["Cut-Extrude1"], "direction":"x", "count":"holes",
               "up_to":{"point":{"at":[50,50,0]}}, "up_to_offset":5, "up_to_keeps":"count"}),
    );
    assert_holes(&doc, 3);
    ok(
        &mut doc,
        json!({"op":"set_parameter", "name":"holes", "value":"3.4"}),
    );
    assert_holes(&doc, 3);
}

#[test]
fn a_seed_that_left_no_faces_of_its_own_is_where_its_sketch_puts_it() {
    // A sheet cut on a hole already there makes no faces of its own (SolidWorks' Gland
    // Plate patterns a hole that sits on another pattern's corner hole): it is patterned
    // from where its sketch puts it, as the first cut is from its faces
    let sheet = |seed: &str| {
        let mut doc = Document::default();
        for op in [
            json!({"op":"sketch", "on":"top", "draw":[{"type":"rectangle", "from":[-50,-50], "to":[50,50]}]}),
            json!({"op":"base_flange", "sketch":"Sketch1", "thickness":4}),
            json!({"op":"sketch", "name":"First", "on":{"feature":"Base-Flange1", "side":"top"}, "draw":[{"type":"circle", "center":[15,0], "radius":1}]}),
            json!({"op":"sheet_cut", "name":"First cut", "sketch":"First"}),
            json!({"op":"sketch", "name":"Again", "on":{"feature":"Base-Flange1", "side":"top"}, "draw":[{"type":"circle", "center":[15,0], "radius":1}]}),
            json!({"op":"sheet_cut", "name":"Again cut", "sketch":"Again"}),
            json!({"op":"plane", "from":"right", "distance":40, "name":"End"}),
        ] {
            ok(&mut doc, op);
        }
        assert_holes(&doc, 1);
        ok(
            &mut doc,
            json!({"op":"linear_pattern", "features":[seed], "direction":"x", "spacing":5,
                   "up_to":{"plane":"End"}, "up_to_offset":2}),
        );
        let status = ok(&mut doc, json!({"op":"status"}));
        assert!(
            status["failures"].as_array().is_none_or(Vec::is_empty),
            "{status}"
        );
        holes(&doc)
    };
    let copied = sheet("First cut");
    assert!(copied > 2.0, "{copied} holes");
    let again = sheet("Again cut");
    assert!(
        (again - copied).abs() < 1e-6,
        "{again} holes, where the first cut's pattern makes {copied}"
    );
}

#[test]
fn a_reference_behind_the_seed_says_so() {
    let mut doc = plate();
    let reply = apply_json(
        &mut doc,
        &json!({"op":"linear_pattern", "features":["Cut-Extrude1"], "direction":"x", "flip":true,
                "spacing":5, "up_to":{"plane":"End"}}),
        Undo::Step,
    );
    assert!(
        reply.json.to_string().contains("flip the direction"),
        "{}",
        reply.json
    );
}

#[test]
fn up_to_fields_need_up_to() {
    let mut doc = plate();
    let reply = apply_json(
        &mut doc,
        &json!({"op":"linear_pattern", "features":["Cut-Extrude1"], "direction":"x", "spacing":5,
                "count":2, "up_to_offset":3}),
        Undo::Step,
    );
    assert!(!reply.ok);
    assert!(
        reply.json.to_string().contains("give 'up_to'"),
        "{}",
        reply.json
    );
}

#[test]
fn no_room_for_a_copy_says_so() {
    // As a count of 1 does. (SolidWorks' Gear tray has such a pattern, count 1, suppressed
    // in every configuration.)
    let mut doc = plate();
    let reply = apply_json(
        &mut doc,
        &json!({"op":"linear_pattern", "features":["Cut-Extrude1"], "direction":"x", "spacing":30,
                "up_to":{"plane":"End"}}),
        Undo::Step,
    );
    assert!(
        reply.json.to_string().contains("no room for a copy"),
        "{}",
        reply.json
    );
    assert_holes(&doc, 1);
}
