//! Dimensions by SolidWorks-style address `NAME@FEATURE`: reading, setting and naming
//! them, in expressions and per configuration.
use peet_document::Document;
use peet_kernel::validate::measure;
use peet_ops::{Undo, apply_json};
use serde_json::{Value, json};

fn ok(doc: &mut Document, op: Value) -> Value {
    let reply = apply_json(doc, &op, Undo::Step);
    assert!(reply.ok, "{op}: {}", reply.json);
    reply.json
}

fn refused(doc: &mut Document, op: Value) -> String {
    let reply = apply_json(doc, &op, Undo::Step);
    assert!(!reply.ok, "{op}: {}", reply.json);
    reply.json["error"].as_str().unwrap_or_default().to_owned()
}

fn volume(doc: &Document) -> f64 {
    doc.bodies.iter().map(|b| measure::volume(&b.solid)).sum()
}

fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-6, "{a} != {b}");
}

/// A 100 × 50 plate, 4 deep, its width the sketch dimension `Width`, with a hole patterned
/// along it.
fn plate() -> Document {
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op":"sketch", "on":"top", "draw":[
            {"type":"rectangle", "from":[0,0], "to":[100,50], "as":"r"},
            {"type":"coincident", "of":["r.bottom.start", "origin"]},
            {"type":"length", "of":["r.bottom"], "value":100, "name":"Width"},
            {"type":"length", "of":["r.right"], "value":50},
        ]}),
    );
    ok(
        &mut doc,
        json!({"op":"extrude", "sketch":"Sketch1", "depth":4}),
    );
    ok(
        &mut doc,
        json!({"op":"sketch", "name":"Seed", "on":{"feature":"Extrude1", "side":"end"}, "draw":[{"type":"circle", "center":[10,25], "radius":1}]}),
    );
    ok(
        &mut doc,
        json!({"op":"cut", "sketch":"Seed", "end":"through_all"}),
    );
    ok(
        &mut doc,
        json!({"op":"linear_pattern", "features":["Cut-Extrude1"], "direction":"x", "spacing":20, "count":2}),
    );
    doc
}

fn hole() -> f64 {
    4.0 * std::f64::consts::PI
}

#[test]
fn set_reads_and_names_by_address() {
    let mut doc = plate();
    let list = ok(&mut doc, json!({"op":"dimensions"}));
    let addresses: Vec<&str> = list["dimensions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["address"].as_str().unwrap())
        .collect();
    for a in [
        "Width@Sketch1",
        "d2@Sketch1",
        "d1@Extrude1",
        "d1@LPattern1",
        "d2@LPattern1",
    ] {
        assert!(addresses.contains(&a), "{a}: {addresses:?}");
    }
    // A sketch dimension and a feature's value, by address; the field is an address too.
    ok(
        &mut doc,
        json!({"op":"set", "name":"Width@Sketch1", "value":120}),
    );
    ok(
        &mut doc,
        json!({"op":"set", "name":"depth@Extrude1", "value":5}),
    );
    close(volume(&doc), 120.0 * 50.0 * 5.0 - 2.0 * hole() * 5.0 / 4.0);
    // Names as SolidWorks has them, a whole address for a value it keeps elsewhere.
    ok(
        &mut doc,
        json!({"op":"name_dimension", "feature":"LPattern1", "dimension":"count", "name":"D1"}),
    );
    ok(
        &mut doc,
        json!({"op":"name_dimension", "feature":"LPattern1", "dimension":"spacing", "name":"D3@Sketch9"}),
    );
    ok(
        &mut doc,
        json!({"op":"name_dimension", "feature":"Sketch1", "dimension":"d2", "name":"Dia."}),
    );
    // A count worked out from a length reads it by address and is rounded as SolidWorks
    // rounds it: (120 - 25) / 30 = 3.17 copies is 3.
    ok(
        &mut doc,
        json!({"op":"set", "name":"D1@LPattern1", "value":"IIF(\"Width@Sketch1\" > 110, (\"Width@Sketch1\" - 25mm) / 30mm, 2)"}),
    );
    ok(
        &mut doc,
        json!({"op":"set", "name":"D3@Sketch9", "value":"\"Dia.@Sketch1\" / 2"}),
    );
    close(volume(&doc), 120.0 * 50.0 * 5.0 - 3.0 * hole() * 5.0 / 4.0);
    let one = ok(&mut doc, json!({"op":"dimensions", "name":"D3@Sketch9"}));
    assert_eq!(one["dimensions"][0]["field"], "spacing");
    assert_eq!(one["dimensions"][0]["value"]["value"], 25.0);
    // Changing what an expression reads moves what follows it.
    ok(
        &mut doc,
        json!({"op":"set", "name":"Width@Sketch1", "value":100}),
    );
    close(volume(&doc), 100.0 * 50.0 * 5.0 - 2.0 * hole() * 5.0 / 4.0);
    assert!(
        refused(
            &mut doc,
            json!({"op":"set", "name":"Height@Sketch1", "value":3})
        )
        .contains("No dimension")
    );
    assert!(
        refused(&mut doc, json!({"op":"set", "name":"nothing", "value":3}))
            .contains("no parameter")
    );
    assert!(
        refused(
            &mut doc,
            json!({"op":"name_dimension", "feature":"Sketch1", "dimension":"Width", "name":"a@b"})
        )
        .contains("'@'")
    );
}

#[test]
fn set_takes_parameters_and_configurations() {
    let mut doc = plate();
    ok(
        &mut doc,
        json!({"op":"set_parameter", "name":"t", "value":"4mm"}),
    );
    ok(
        &mut doc,
        json!({"op":"set", "name":"depth@Extrude1", "value":"t"}),
    );
    ok(&mut doc, json!({"op":"add_configuration", "name":"Long"}));
    ok(
        &mut doc,
        json!({"op":"set", "name":"Width@Sketch1", "value":150, "configurations":"this"}),
    );
    ok(&mut doc, json!({"op":"set", "name":"t", "value":"6mm"}));
    close(volume(&doc), 150.0 * 50.0 * 6.0 - 2.0 * hole() * 6.0 / 4.0);
    let width = ok(&mut doc, json!({"op":"dimensions", "name":"Width@Sketch1"}));
    assert_eq!(
        width["dimensions"][0]["configurations"],
        json!({"Default": 100.0, "Long": 150.0})
    );
    ok(
        &mut doc,
        json!({"op":"configuration", "configuration":"Default"}),
    );
    close(volume(&doc), 100.0 * 50.0 * 4.0 - 2.0 * hole());
    // A suppression rule reads a dimension by address.
    ok(
        &mut doc,
        json!({"op":"set_suppression_expression", "feature":"LPattern1", "value":"iif(\"Width@Sketch1\" > 120, 0, 1)"}),
    );
    close(volume(&doc), 100.0 * 50.0 * 4.0 - hole());
    ok(
        &mut doc,
        json!({"op":"configuration", "configuration":"Long"}),
    );
    close(volume(&doc), 150.0 * 50.0 * 6.0 - 2.0 * hole() * 6.0 / 4.0);
}

/// The angle between two lines on the other side, as SolidWorks shows one whose dimension
/// is drawn there: `supplementary_angle` holds the value SolidWorks has, and setting it
/// turns the line as SolidWorks would.
#[test]
fn a_supplementary_angle_holds_the_value_solidworks_shows() {
    let mut doc = Document::default();
    let reply = ok(
        &mut doc,
        json!({"op":"sketch", "on":"top", "name":"Chamfer", "draw":[
            {"type":"line", "from":[0,0], "to":[40,0], "as":"base"},
            {"type":"line", "from":[40,0], "to":[60,20], "as":"slope"},
            {"type":"fix", "of":["base"]},
            {"type":"coincident", "of":["base.end", "slope.start"]},
            {"type":"length", "of":["slope"], "value":20},
            {"type":"supplementary_angle", "of":["base", "slope"], "value":135, "name":"bevel"},
        ]}),
    );
    assert_eq!(reply["definition"], "fully_defined", "{reply}");
    let slope = |doc: &mut Document| {
        let f = ok(doc, json!({"op":"feature", "feature":"Chamfer"}));
        let line = f["entities"].as_array().unwrap()[1].clone();
        let (a, b) = (&line["from"], &line["to"]);
        let d = [
            b[0].as_f64().unwrap() - a[0].as_f64().unwrap(),
            b[1].as_f64().unwrap() - a[1].as_f64().unwrap(),
        ];
        (d[1].atan2(d[0]).to_degrees().abs(), f["dimensions"][1]["value"].clone())
    };
    let (turn, value) = slope(&mut doc);
    assert!((turn - 45.0).abs() < 1e-6, "{turn}");
    assert_eq!(value, 135.0);
    ok(
        &mut doc,
        json!({"op":"set", "name":"bevel@Chamfer", "value":120}),
    );
    let (turn, value) = slope(&mut doc);
    assert!((turn - 60.0).abs() < 1e-6, "{turn}");
    assert_eq!(value, 120.0);
}
