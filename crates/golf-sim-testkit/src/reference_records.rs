//! Record decoders for fixtures from the reference implementation.

use golf_sim::{
    records::hole::{DecisionEntry, DecisionKind, Refusal, StrokeFacts, decode_envelope},
    setup::{HoleSetup, PlaySettings},
};
use golf_sim_base::{
    Error,
    variant::{Dict, Variant},
};

#[derive(Clone, Debug, PartialEq)]
pub struct ReferenceHoleRecord {
    pub versions: Dict,
    pub setup: HoleSetup,
    pub entries: Vec<ReferenceEntry>,
    dictionary: Dict,
}

impl ReferenceHoleRecord {
    /// The source dictionary, including its Variant widths and insertion order.
    pub fn dictionary(&self) -> &Dict {
        &self.dictionary
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ReferenceEntry {
    Stroke(Box<StrokeFacts>),
    Decision(DecisionEntry),
}

/// Format 4 stores the course revision in the record and takes the course id from its context.
pub fn decode_hole(bytes: &[u8], course_id: &str) -> Result<ReferenceHoleRecord, Refusal> {
    let dictionary = decode_envelope(bytes)?;
    let versions = dict_field(&dictionary, "versions", "versions")?;
    let written = string_field(versions, "record", "versions.record")?;
    if written != "4" {
        return Err(Refusal::UnsupportedFormat(written.into()));
    }
    for key in ["sim", "stroke", "rules"] {
        string_field(versions, key, &format!("versions.{key}"))?;
    }
    let revision = string_field(&dictionary, "course_revision", "course_revision")?;
    let settings = PlaySettings::from_dict(dict_field(&dictionary, "settings", "settings")?);
    let setup = dict_field(&dictionary, "setup", "setup")?;
    let setup = Dict::new(
        [("course_id".into(), course_id.into())]
            .into_iter()
            .chain(setup.entries().iter().cloned())
            .collect(),
    );
    let setup = HoleSetup::from_dict(&setup, revision.into(), settings)?;
    let Variant::Array(entries) = field(&dictionary, "entries", "entries")? else {
        return Err(wrong_type("entries", "Array").into());
    };
    let entries = entries
        .items
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let path = format!("entries[{index}]");
            let kind_path = format!("{path}.kind");
            let entry = entry
                .as_dict()
                .ok_or_else(|| wrong_type(&path, "Dictionary"))?;
            let kind = string_field(entry, "kind", &kind_path)?;
            if kind == "stroke" {
                let stroke = StrokeFacts::from_dict(entry, index)?;
                return Ok(ReferenceEntry::Stroke(Box::new(stroke)));
            }
            let kind = DecisionKind::from_name(kind).ok_or_else(|| Error::Malformed {
                path: kind_path,
                message: "unknown decision".into(),
            })?;
            let decision = DecisionEntry::from_dict(kind, entry, index)?;
            if decision.option > 10 {
                return Err(Error::Malformed {
                    path: format!("{path}.option"),
                    message: "unknown relief option".into(),
                });
            }
            Ok(ReferenceEntry::Decision(decision))
        })
        .collect::<Result<_, Error>>()?;
    Ok(ReferenceHoleRecord {
        versions: versions.clone(),
        setup,
        entries,
        dictionary,
    })
}

fn field<'a>(dict: &'a Dict, key: &str, path: &str) -> Result<&'a Variant, Error> {
    dict.get(key)
        .ok_or_else(|| Error::MissingKey { path: path.into() })
}

fn dict_field<'a>(dict: &'a Dict, key: &str, path: &str) -> Result<&'a Dict, Error> {
    field(dict, key, path)?
        .as_dict()
        .ok_or_else(|| wrong_type(path, "Dictionary"))
}

fn string_field<'a>(dict: &'a Dict, key: &str, path: &str) -> Result<&'a str, Error> {
    match field(dict, key, path)? {
        Variant::String(value) => Ok(value),
        _ => Err(wrong_type(path, "String")),
    }
}

fn wrong_type(path: &str, expected: &'static str) -> Error {
    Error::WrongType {
        path: path.into(),
        expected,
    }
}
