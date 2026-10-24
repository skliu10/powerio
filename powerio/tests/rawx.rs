use powerio::network::{
    ImpedanceCorrection, ImpedanceCorrectionPoint, ImpedanceCorrectionVariable,
};
use powerio::{
    Destination, EmittedOutput, FormatId, PioValue, Source, deserialize, emit, parse, serialize,
};

const RAWX: &str = r#"{
  "network": {
    "caseid": {"fields":["rev","sbase","basfrq","title1"],"data":[35,100,60,"rawx-public"]},
    "bus": {"fields":["ibus","name","baskv","ide","area","zone","owner","vm","va"],"data":[[1,"Slack",230,3,1,1,1,1,0],[2,"Load",230,1,1,1,1,1,-1]]},
    "load": {"fields":["ibus","loadid","stat","pl","ql","ip","iq","yp","yq"],"data":[[2,"L1",1,20,5,1,2,3,4]]},
    "generator": {"fields":["ibus","machid","pg","qg","qt","qb","vs","ireg","nreg","mbase","stat","pt","pb"],"data":[[1,"G1",20,5,50,-50,1,2,0,100,1,80,0]]},
    "acline": {"fields":["ibus","jbus","ckt","rpu","xpu","bpu","rate1","stat"],"data":[[1,2,"1",0.01,0.1,0.02,100,1]]}
  }
}"#;

const RAWX_FIDELITY: &str = r#"{
  "network": {
    "caseid": {"fields":["rev","sbase","basfrq","title1"],"data":[35,100,60,"rawx-fidelity"]},
    "bus": {"fields":["ibus","name","baskv","ide","area","zone","owner","vm","va"],"data":[[1,"Slack",230,3,1,1,1,1,0]]},
    "generator": {
      "fields":["ibus","machid","pg","qg","qt","qb","vs","ireg","nreg","mbase","zr","zx","rt","xt","gtap","stat","rmpct","pt","pb","baslod","o1","f1","o2","f2","o3","f3","o4","f4","wmod","wpf"],
      "data":[[1,"A",20,5,50,-50,1,0,0,100,0.0019,0.29,0.02,0.31,1.03,1,87.5,80,0,3,2,0.75,3,0.25,0,1,0,1,1,0.97]]
    },
    "swshunt": {
      "fields":["ibus","shntid","modsw","adjm","stat","vswhi","vswlo","swreg","nreg","rmpct","rmidnt","binit","s1","n1","b1"],
      "data":[[1,"S1",2,1,1,1.05,0.95,0,0,100,"",0,1,1,10]]
    },
    "sub": {"fields":["isub","name","lati","long","srg"],"data":[[1,"SUB",null,null,null]]},
    "subnode": {
      "fields":["isub","inode","name","ibus","stat","vm","va"],
      "data":[[1,1,"UNSET",1,1,null,null],[1,2,"ASSIGNED",1,1,0.98,-1.5]]
    }
  }
}"#;

fn memory_text(result: powerio::EmitResult) -> String {
    let EmittedOutput::Memory { mut artifacts } = result.into_output() else {
        panic!("memory destination returned a path output");
    };
    String::from_utf8(artifacts.pop().unwrap().into_bytes()).unwrap()
}

fn network_with_correction(correction: ImpedanceCorrection) -> powerio::BalancedNetwork {
    let module =
        parse(Source::from_memory("case.rawx", RAWX.as_bytes().to_vec()).unwrap()).unwrap();
    let PioValue::BalancedNetwork(network) = module.value() else {
        panic!("balanced network expected")
    };
    let mut network = network.clone();
    network.branches_mut()[0].tap = 1.05 / 0.98;
    network.branches_mut()[0]
        .extras
        .insert("psse_winding2_ratio".into(), 0.98.into());
    network.branches_mut()[0].set_impedance_correction(Some(correction));
    network
}

#[test]
fn single_point_correction_emits_an_equivalent_constant_table() {
    for format in ["psse", "psse35", "psse-rawx"] {
        for variable in [
            ImpedanceCorrectionVariable::Ratio,
            ImpedanceCorrectionVariable::Angle,
        ] {
            let imaginary = if format == "psse" { 0.0 } else { 0.1 };
            let mut network = network_with_correction(ImpedanceCorrection::new(
                7,
                variable,
                vec![ImpedanceCorrectionPoint::new(0.8, 1.2, imaginary)],
            ));
            network.apply_impedance_correction();
            let expected = (network.branches()[0].r, network.branches()[0].x);
            let module = powerio::PioModule::new(PioValue::BalancedNetwork(network));
            let text =
                memory_text(emit(&module, format, Destination::memory("fresh").unwrap()).unwrap());
            let restored = parse(
                Source::from_memory("fresh", text.into_bytes())
                    .unwrap()
                    .with_format(FormatId::new(format).unwrap()),
            )
            .unwrap();
            let PioValue::BalancedNetwork(back) = restored.value() else {
                panic!("balanced network expected")
            };
            let correction = back.branches()[0].impedance_correction().unwrap();
            assert_eq!(correction.variable, variable);
            assert_eq!(correction.points.len(), 2);
            for at in [-60.0, 0.5, 0.8, 1.0, 1.5, 60.0] {
                assert_eq!(correction.calc_factor(at), Some((1.2, imaginary)));
            }
            let mut back = back.clone();
            back.apply_impedance_correction();
            assert!((back.branches()[0].r - expected.0).abs() < 1e-12);
            assert!((back.branches()[0].x - expected.1).abs() < 1e-12);
        }
    }
}

#[test]
fn singleton_source_corrections_preserve_the_constant_case_on_fresh_output() {
    for format in ["psse", "psse34", "psse35", "psse-rawx"] {
        for (variable, at) in [
            (ImpedanceCorrectionVariable::Ratio, 0.8),
            (ImpedanceCorrectionVariable::Angle, -30.0),
        ] {
            let imaginary = if format == "psse" { 0.0 } else { 0.1 };
            let network = network_with_correction(ImpedanceCorrection::new(
                7,
                variable,
                vec![ImpedanceCorrectionPoint::new(at, 1.2, imaginary)],
            ));
            let expected = (
                network.branches()[0].r * 1.2 - network.branches()[0].x * imaginary,
                network.branches()[0].r * imaginary + network.branches()[0].x * 1.2,
            );
            let module = powerio::PioModule::new(PioValue::BalancedNetwork(network));
            let mut source =
                memory_text(emit(&module, format, Destination::memory("case").unwrap()).unwrap());
            if format == "psse-rawx" {
                let mut root: serde_json::Value = serde_json::from_str(&source).unwrap();
                root["network"]["impcor"]["data"] = serde_json::json!([[7, at, 1.2, imaginary]]);
                source = root.to_string();
            } else {
                let record = source
                    .lines()
                    .find(|line| line.trim_start().starts_with("7,"))
                    .unwrap();
                let singleton = if format == "psse" {
                    format!("7, {at}, 1.2, 0.0, 0.0")
                } else {
                    format!("7, {at}, 1.2, {imaginary}, 0.0, 0.0, 0.0")
                };
                source = source.replace(record, &singleton);
            }
            let read = |text: String| {
                let module = parse(
                    Source::from_memory("case", text.into_bytes())
                        .unwrap()
                        .with_format(FormatId::new(format).unwrap()),
                )
                .unwrap();
                let PioValue::BalancedNetwork(network) = module.into_value() else {
                    panic!("balanced network expected")
                };
                network
            };
            let network = read(source);
            let correction = network.branches()[0].impedance_correction().unwrap();
            assert_eq!(correction.variable, variable);
            assert_eq!(correction.points.len(), 1);
            for applied in [false, true] {
                let mut network = network.clone();
                if applied {
                    network.apply_impedance_correction();
                    assert!((network.branches()[0].r - expected.0).abs() < 1e-12);
                    assert!((network.branches()[0].x - expected.1).abs() < 1e-12);
                }
                let module = powerio::PioModule::new(PioValue::BalancedNetwork(network));
                let fresh = memory_text(
                    emit(&module, format, Destination::memory("fresh").unwrap()).unwrap(),
                );
                let mut back = read(fresh);
                let correction = back.branches()[0].impedance_correction().unwrap();
                assert_eq!(correction.variable, variable);
                assert_eq!(correction.points.len(), 2);
                for at in [-60.0, 0.5, 0.8, 1.0, 1.5, 60.0] {
                    assert_eq!(correction.calc_factor(at), Some((1.2, imaginary)));
                }
                back.apply_impedance_correction();
                assert!((back.branches()[0].r - expected.0).abs() < 1e-12);
                assert!((back.branches()[0].x - expected.1).abs() < 1e-12);
            }
        }
    }
}

#[test]
fn oversized_correction_tables_refuse_fresh_output_before_losing_case_state() {
    for (format, count) in [("psse", 12), ("psse35", 100), ("psse-rawx", 100)] {
        for applied in [false, true] {
            let points = (0..count)
                .map(|index| {
                    ImpedanceCorrectionPoint::new(
                        0.9 + f64::from(index) * 0.002,
                        1.0 + f64::from(index) * 0.001,
                        0.0,
                    )
                })
                .collect();
            let mut network = network_with_correction(ImpedanceCorrection::new(
                7,
                ImpedanceCorrectionVariable::Ratio,
                points,
            ));
            if applied {
                network.apply_impedance_correction();
            }
            let module = powerio::PioModule::new(PioValue::BalancedNetwork(network));
            let error = emit(&module, format, Destination::memory("fresh").unwrap()).unwrap_err();
            assert!(
                error.to_string().contains("impedance correction table 7"),
                "{error}"
            );
            assert!(error.to_string().contains("must contain"), "{error}");
        }
    }
}

#[test]
fn correction_tables_and_winding2_ratio_survive_storage_and_normalization() {
    let mut root: serde_json::Value = serde_json::from_str(RAWX).unwrap();
    root["network"]["transformer"] = serde_json::json!({
        "fields": ["ibus", "jbus", "kbus", "ckt", "cw", "cz", "r1_2", "x1_2", "windv1", "windv2", "tab1"],
        "data": [[1, 2, 0, "T1", 1, 1, 0.02, 0.2, 1.05, 0.98, 7]]
    });
    root["network"]["impcor"] = serde_json::json!({
        "fields": ["itable", "tap", "refact", "imfact"],
        "data": [[7, 0.9, 1.1, 0.1], [7, 1.1, 1.2, -0.1]]
    });
    let module =
        parse(Source::from_memory("correction.rawx", root.to_string().into_bytes()).unwrap())
            .unwrap();
    let stored = memory_text(
        serialize(&module, Destination::memory("correction.pio.json").unwrap()).unwrap(),
    );
    let restored =
        deserialize(Source::from_memory("correction.pio.json", stored.into_bytes()).unwrap())
            .unwrap();
    let PioValue::BalancedNetwork(network) = restored.value() else {
        panic!("balanced network expected")
    };
    let branch = &network.branches()[1];
    assert_eq!(branch.extras["psse_winding2_ratio"], 0.98);
    let correction = branch.impedance_correction().unwrap();
    let normalized = network.to_normalized().unwrap();
    assert_eq!(normalized.branches()[1].extras["psse_winding2_ratio"], 0.98);
    assert_eq!(
        normalized.branches()[1].impedance_correction(),
        Some(correction.clone())
    );
    let fresh = memory_text(
        emit(
            &restored,
            "psse-rawx",
            Destination::memory("fresh.rawx").unwrap(),
        )
        .unwrap(),
    );
    let reparsed = parse(Source::from_memory("fresh.rawx", fresh.into_bytes()).unwrap()).unwrap();
    let PioValue::BalancedNetwork(back) = reparsed.value() else {
        panic!("balanced network expected")
    };
    assert_eq!(back.branches()[1].extras["psse_winding2_ratio"], 0.98);
    let recovered = back.branches()[1].impedance_correction().unwrap();
    for (a, b) in recovered.points.iter().zip(correction.points) {
        assert!((a.value - b.value).abs() < 1e-12);
        assert!((a.factor_re - b.factor_re).abs() < 1e-12);
        assert!((a.factor_im - b.factor_im).abs() < 1e-12);
    }
}

#[test]
fn universal_parse_normalizes_rawx_metadata_and_echoes_exactly() {
    let source = Source::from_memory("input.json", RAWX.as_bytes().to_vec())
        .unwrap()
        .with_format(FormatId::new("rawx").unwrap());
    let module = parse(source).unwrap();
    assert_eq!(
        module.source().unwrap().format().unwrap().as_str(),
        "psse-rawx"
    );
    let PioValue::BalancedNetwork(network) = &module.value() else {
        panic!("RAWX did not produce a balanced network");
    };
    assert!((network.loads()[0].p - 24.0).abs() < f64::EPSILON);
    // YQ 4 is capacitive: Q = QL + IQ - YQ.
    assert!((network.loads()[0].q - 3.0).abs() < f64::EPSILON);

    let error = emit(&module, "rawx", Destination::memory("same.rawx").unwrap()).unwrap_err();
    assert_eq!(
        error.info().map(|info| info.code),
        Some("REQUEST.EMIT.UNKNOWN_FORMAT")
    );

    let result = emit(
        &module,
        "psse-rawx",
        Destination::memory("same.rawx").unwrap(),
    )
    .unwrap();
    assert_eq!(memory_text(result), RAWX);
}

#[test]
fn matpower_cross_format_output_reloads_as_rawx() {
    let matpower = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case9.m");
    let module = parse(Source::open(matpower).unwrap()).unwrap();
    let result = emit(
        &module,
        "psse-rawx",
        Destination::memory("case9.rawx").unwrap(),
    )
    .unwrap();
    let text = memory_text(result);
    let root: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(root["network"]["caseid"]["data"][2], 35);

    let back = parse(Source::from_memory("case9.rawx", text.into_bytes()).unwrap()).unwrap();
    let PioValue::BalancedNetwork(network) = &back.value() else {
        panic!("RAWX did not produce a balanced network");
    };
    assert_eq!(network.buses().len(), 9);
    assert_eq!(network.branches().len(), 9);
}

#[test]
fn ir_fresh_emission_preserves_adjm_and_null_subnode_voltage() {
    let source = Source::from_memory("input.rawx", RAWX_FIDELITY.as_bytes().to_vec())
        .unwrap()
        .with_format(FormatId::new("psse-rawx").unwrap());
    let module = parse(source).unwrap();

    let stored = serialize(&module, Destination::memory("network.pio.json").unwrap()).unwrap();
    let stored = memory_text(stored);
    let restored =
        deserialize(Source::from_memory("network.pio.json", stored.into_bytes()).unwrap()).unwrap();
    let emitted = emit(
        &restored,
        "psse-rawx",
        Destination::memory("fresh.rawx").unwrap(),
    )
    .unwrap();
    let root: serde_json::Value = serde_json::from_str(&memory_text(emitted)).unwrap();

    let generator = &root["network"]["generator"];
    let generator_fields = generator["fields"].as_array().unwrap();
    let generator_row = generator["data"][0].as_array().unwrap();
    let generator_value = |field: &str| {
        let column = generator_fields
            .iter()
            .position(|candidate| candidate == field)
            .unwrap();
        &generator_row[column]
    };
    assert_eq!(generator_value("machid"), "A");
    assert_eq!(generator_value("zr"), 0.0019);
    assert_eq!(generator_value("zx"), 0.29);
    assert_eq!(generator_value("rt"), 0.02);
    assert_eq!(generator_value("xt"), 0.31);
    assert_eq!(generator_value("gtap"), 1.03);
    assert_eq!(generator_value("rmpct"), 87.5);
    assert_eq!(generator_value("baslod"), 3);
    assert_eq!(generator_value("o1"), 2);
    assert_eq!(generator_value("f1"), 0.75);
    assert_eq!(generator_value("o2"), 3);
    assert_eq!(generator_value("f2"), 0.25);
    assert_eq!(generator_value("wmod"), 1);
    assert_eq!(generator_value("wpf"), 0.97);
    assert_eq!(root["network"]["swshunt"]["data"][0][3], 1);
    assert!(root["network"]["subnode"]["data"][0][5].is_null());
    assert!(root["network"]["subnode"]["data"][0][6].is_null());
    assert_eq!(root["network"]["subnode"]["data"][1][5], 0.98);
    assert_eq!(root["network"]["subnode"]["data"][1][6], -1.5);
}
